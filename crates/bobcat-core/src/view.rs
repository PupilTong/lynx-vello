//! A running Lynx view: the opaque [`LynxView`] an embedder holds, and the
//! private pipeline behind it.
//!
//! An embedder provides exactly five things: user input, device metrics,
//! OS initialization (its event loop and window), a draw target, and IO
//! primitives (resource bytes in, pixels out). It never starts or steers the
//! internal pipeline. A view's own sources are named once, in [`ViewSources`],
//! and [`LynxView::new`] applies them; after that the embedder's event
//! handlers are relays — they hand the view an OS fact (`dispatch_input`,
//! `resize`, `draw`, `pump`) and it decides what the pipeline does with it.
//! One wakeup carries everything the engine has to say: a frame it wants
//! drawn and a lifecycle event both wake the host event loop through
//! [`EventRequester`], and the handler that answers calls `draw` and `pump`.
//!
//! # One committer, one compositor
//!
//! The document exists in exactly one place for its whole life: the
//! view-owned Lynx main thread — the same thread that runs the `QuickJS`
//! realm. [`LynxView::new`] builds the document, applies every source to it,
//! and hands it to that thread, which boots the entry module and then serves
//! commands forever; the presenting side never holds it. Every later change
//! reaches the document as a [`MainCommand`] on the one ordered channel
//! (input targets, refill write-backs, resizes, `BeginFrame` animation
//! ticks). That thread is the only committer: `__FlushElementTree` commits,
//! and every served command round ends in a commit when anything went stale.
//!
//! What crosses back is one immutable [`CommittedFrame`] per commit,
//! published into the [`FrameHub`]. The presenting side is a pure compositor
//! over it: `draw` acquires the swap-chain image, uploads the
//! latest published scene if it is new, and presents — no document, no lock,
//! no skipped frames. Input is routed against the published frame too: hit
//! testing and gesture recognition read its tables, and scroll consumption
//! lands in the presenting side's own intents — the offsets the screen
//! shows, written back to the document only when a refill re-centers the
//! encode windows.
//!
//! The law: the main thread waits only on its own channel; the presenting
//! side never waits on the main thread (the offscreen `tick`, an embedder's
//! synthetic vsync, is the deliberate exception — it is a synchronization
//! point, not a frame path); nothing waits on an OS frame callback — a
//! commit wakes the host loop directly and the next `draw` is the frame; the
//! frame's vsync wait — the swap-chain acquire that opens a window frame —
//! happens before any of the frame's work, so the clock reading everything
//! resolves against belongs to the frame being shown.

mod graphics;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod animation_tests;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;
use std::time::Duration;

use dom::input::{InputEvent, InputKind};
use dom::render::gpu::Headless;
use dom::scroll::ScrollAxes;
use dom::vello::Scene;
use dom::vello::peniko::Color;
use dom::{CommittedFrame, FontBlob, HitTarget, ImageStore, NodeId, Vector2D};
use http::HeaderMap;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

pub use self::graphics::WindowTarget;
use self::graphics::{FrameAcquisition, WindowGraphics};
use crate::clock::FrameClock;
use crate::gesture::{EmitEvent, GestureRouter, InputDecision, RouterHost};
use crate::resource::{
    CachePolicy, RequestContext, RequestId, ResolveRequest, ResourceDescriptor, ResourceFetcher,
    ResourcePriority, ResourceRequest, StyleSheetPayload,
};
use crate::runtime::MainThreadError;
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
use crate::tree::{LynxDocument, PageConfig, Viewport, new_document};

static NEXT_REQUEST_NAMESPACE: AtomicU64 = AtomicU64::new(1);

/// The physical pixel size of the render target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

const MAX_RENDER_DIMENSION: u32 = 16_384;

/// What one drawn frame is a function of: the commit id and the
/// scroll-intents generation composed with it. Same key, same pixels.
type ComposeKey = (u64, u64);
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
    #[error("no registered or system font family is named `{0}`")]
    UnknownFontFamily(String),
    #[error("the engine has no draw target attached")]
    NoDrawTarget,
    #[error("the document is busy in a script batch; retry the repaint request")]
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

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// The entry MTS module and Bobcat boot completed successfully.
    ScriptFinished,
    /// The script runtime failed fatally during boot or later owner-thread work.
    ScriptRunError(ScriptError),
    /// A listener threw while an event was being delivered to it.
    ///
    /// Reported rather than swallowed, and separate from [`Self::ScriptRunError`]
    /// because it is not fatal: the walk goes on, the realm stays usable, and
    /// every later event is delivered as normal.
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

/// The embedder's window: the draw target it lends, and nothing else. The
/// embedder provides the surface; the engine decides when to draw into it.
///
/// The draw target is a GAT, which lets native surfaces borrow an
/// embedder-owned window. Browser embedders can instead attach an owned
/// canvas target through [`crate::LynxView::attach_target`].
pub trait Window {
    type Target<'window>: Into<WindowTarget<'window>>
    where
        Self: 'window;

    fn target(&self) -> Self::Target<'_>;
}

/// The host event-loop capability the engine wakes, and the only one it has.
///
/// Both of the engine's outward signals ride it: a lifecycle event to drain
/// through [`crate::LynxView::pump`], and a frame to draw through
/// [`crate::LynxView::draw`]. Engine-owned threads record the fact before
/// invoking this callback, so an embedder may service both the moment its
/// event loop receives the wakeup. Nothing here is a draw target: lifecycle
/// progress must not depend on a visible or attached one.
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

/// The frame wakeup: the pending-frame bit and the host event-loop
/// capability that carries it.
///
/// A frame is asked for by setting the bit and waking the loop the embedder
/// already runs — the same wakeup lifecycle events use. No OS frame callback
/// stands between the request and the pixels: the handler that answers the
/// wakeup calls [`LynxView::draw`], which takes the bit. The committer thread
/// holds one too, so a commit reaches the screen without asking a window for
/// anything.
#[derive(Clone)]
struct FrameWakeup {
    pending: Arc<AtomicBool>,
    requester: Arc<dyn EventRequester>,
}

impl FrameWakeup {
    fn request(&self) {
        // Record first: after this wakeup, `draw` must be able to observe the
        // pending frame without a polling race.
        self.pending.store(true, Ordering::Release);
        self.requester.request_event();
    }

    /// Takes the pending frame, if one was asked for.
    fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
}

#[derive(Clone)]
struct EngineEventSender {
    sender: mpsc::Sender<EngineEvent>,
    requester: Arc<dyn EventRequester>,
}

impl EngineEventSender {
    fn send(&self, event: EngineEvent) {
        if self.sender.send(event).is_ok() {
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

    fn target(&self) -> Self::Target<'_> {
        match *self {}
    }
}

/// How long a synchronizing caller waits for the main thread to service a
/// `BeginFrame` before proceeding with whatever frame is published. Generous,
/// because it only ever expires when the main thread is wedged or gone.
const BEGIN_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// The published-frame slot the committer fills and the compositor reads,
/// plus the `BeginFrame` service marker a synchronizing caller waits on.
#[derive(Debug, Default)]
pub(crate) struct FrameHub {
    latest: Mutex<Option<Arc<CommittedFrame>>>,
    begin_frames: Mutex<BeginFrameLedger>,
    begin_frame_signal: Condvar,
}

#[derive(Debug, Default)]
struct BeginFrameLedger {
    serviced: u64,
    /// The committer thread has exited — no queued `BeginFrame` will ever be
    /// serviced, so waiters must stop waiting.
    committer_gone: bool,
}

impl FrameHub {
    pub(crate) fn publish(&self, frame: Arc<CommittedFrame>) {
        *self
            .latest
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}")) = Some(frame);
    }

    pub(crate) fn latest(&self) -> Option<Arc<CommittedFrame>> {
        self.latest
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"))
            .clone()
    }

    /// The committer marks a serviced `BeginFrame`, after the commit that
    /// round produced was published.
    pub(crate) fn note_begin_frame_serviced(&self, seq: u64) {
        let mut ledger = self
            .begin_frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
        ledger.serviced = seq.max(ledger.serviced);
        self.begin_frame_signal.notify_all();
    }

    /// The committer thread is exiting — normally or by panic. Wakes every
    /// waiter so a `BeginFrame` sent while the thread was still alive does
    /// not sleep out its whole timeout.
    pub(crate) fn note_committer_gone(&self) {
        let mut ledger = self
            .begin_frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
        ledger.committer_gone = true;
        self.begin_frame_signal.notify_all();
    }

    /// Waits until the committer has serviced `BeginFrame` number `seq`, or
    /// the committer is gone, or the timeout passes (a wedged committer must
    /// not hang the caller). Returns whether the frame was serviced.
    fn wait_begin_frame(&self, seq: u64, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut ledger = self
            .begin_frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
        while ledger.serviced < seq {
            if ledger.committer_gone {
                return false;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _) = self
                .begin_frame_signal
                .wait_timeout(ledger, deadline - now)
                .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
            ledger = guard;
        }
        true
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

/// The facts the router asks for while deciding, answered from the published
/// frame's scroll-slot table and the shared listener-name table. No document
/// anywhere.
struct FrameRouterHost<'a> {
    frame: Option<&'a CommittedFrame>,
    listener_names: &'a SharedListenerNames,
}

impl RouterHost for FrameRouterHost<'_> {
    fn nearest_user_scrollable(&self, from: HitTarget, axes: ScrollAxes) -> Option<NodeId> {
        let frame = self.frame?;
        let slot = frame.nearest_user_scrollable(from.scroll, axes)?;
        Some(frame.scroll_slots()[slot as usize].node)
    }

    fn contains_node(&self, node: NodeId) -> bool {
        self.frame
            .is_some_and(|frame| frame.slot_of(node).is_some())
    }

    fn has_listener(&self, name: &str) -> bool {
        self.listener_names.contains(name)
    }
}

/// The presenting side's scroll offsets — the offsets the screen is
/// showing, and the only place a user scroll lands between refills.
///
/// Each consumed step accumulates here against the published frame's
/// bounds; composition and hit testing read these as overrides over the
/// committed offsets. The document learns them only when a refill writes
/// them back, so reconciliation is a value comparison: when a published
/// frame's own offset equals the intent, the intent has served its purpose
/// and drops; anything else is kept and re-clamped to the new bounds — a
/// drag held at a clamp is not re-granted headroom the geometry refused.
#[derive(Debug, Default)]
struct ScrollIntents {
    offsets: HashMap<NodeId, Vector2D<f32>>,
    /// The commit id last rebased against, so one publish rebases once.
    rebased_commit: Option<u64>,
    /// Bumped whenever a chain step lands, so composition can key on
    /// "this frame at these offsets" instead of the commit id alone.
    generation: u64,
}

impl ScrollIntents {
    fn rebase(&mut self, frame: &CommittedFrame) {
        if self.rebased_commit == Some(frame.commit_id()) {
            return;
        }
        self.rebased_commit = Some(frame.commit_id());
        self.offsets.retain(|node, offset| {
            let Some(slot) = frame.slot_of(*node) else {
                return false;
            };
            let slot = &frame.scroll_slots()[slot as usize];
            *offset = Vector2D::new(
                clamp_scroll_axis(offset.x, slot.max_offset.x),
                clamp_scroll_axis(offset.y, slot.max_offset.y),
            );
            // Equal means the committed composition already shows this
            // intent — a refill wrote it back, or the clamp converged them.
            *offset != slot.offset
        });
    }

    /// Mirrors the user-agent scroll chain against published geometry:
    /// chained, clamped, per-axis masked. Returns whether anything was
    /// consumed — the fact the gesture router claims a sequence on.
    fn chain(&mut self, frame: &CommittedFrame, from: NodeId, delta: Vector2D<f32>) -> bool {
        self.rebase(frame);
        let slots = frame.scroll_slots();
        let Some(start) = frame.slot_of(from) else {
            return false;
        };
        let mut search = Some(start);
        let mut remaining = delta;
        let mut consumed = false;
        loop {
            let axes = ScrollAxes {
                x: remaining.x != 0.0,
                y: remaining.y != 0.0,
            };
            let Some(index) = frame.nearest_user_scrollable(search, axes) else {
                break;
            };
            let slot = slots[index as usize];
            let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
            let admitted = Vector2D::new(
                if slot.user_scrollable.x {
                    remaining.x
                } else {
                    0.0
                },
                if slot.user_scrollable.y {
                    remaining.y
                } else {
                    0.0
                },
            );
            let applied = Vector2D::new(
                clamp_scroll_axis(offset.x + admitted.x, slot.max_offset.x),
                clamp_scroll_axis(offset.y + admitted.y, slot.max_offset.y),
            );
            let step = applied - offset;
            if step != Vector2D::zero() {
                self.offsets.insert(slot.node, applied);
                remaining -= step;
                consumed = true;
            }
            if remaining == Vector2D::zero() {
                break;
            }
            search = slot.parent;
            if search.is_none() {
                break;
            }
        }
        if consumed {
            self.generation += 1;
        }
        consumed
    }

    /// The intent offset for one scroller, if a scroll is in flight on it —
    /// the compositor's and hit tester's override over the committed one.
    fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>> {
        self.offsets.get(&node).copied()
    }

    /// Whether any offset is currently overridden — when none is, the
    /// frame's own committed composition is exact.
    fn overrides_any(&self) -> bool {
        !self.offsets.is_empty()
    }

    /// Whether an in-flight offset has consumed more than half of its slot's
    /// remaining encode-window headroom on the side it is moving toward —
    /// the cue to ask for a refill commit before the window runs out.
    fn refill_due(&self, frame: &CommittedFrame) -> bool {
        self.offsets.iter().any(|(node, offset)| {
            frame.slot_of(*node).is_some_and(|index| {
                let slot = &frame.scroll_slots()[index as usize];
                let (low, high) = slot.encode_window();
                axis_refill_due(offset.x, slot.offset.x, low.x, high.x)
                    || axis_refill_due(offset.y, slot.offset.y, low.y, high.y)
            })
        })
    }

    /// Every in-flight offset, for a refill's write-back.
    fn writeback(&self) -> Vec<(NodeId, Vector2D<f32>)> {
        self.offsets
            .iter()
            .map(|(node, offset)| (*node, *offset))
            .collect()
    }
}

/// One axis of [`ScrollIntents::refill_due`]: past halfway between the
/// committed offset and the window edge being approached.
fn axis_refill_due(pending: f32, committed: f32, low: f32, high: f32) -> bool {
    if pending < committed {
        pending - low < (committed - low) / 2.0
    } else if pending > committed {
        high - pending < (high - committed) / 2.0
    } else {
        false
    }
}

fn clamp_scroll_axis(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max)
    } else {
        0.0
    }
}

/// The scene to draw for `frame`: when scrolls are in flight, the frame
/// recomposed into `buffer` at their offsets; otherwise the frame's own
/// committed composition, materialized once and shared.
///
/// A free function over the exact fields it reads, so callers holding a
/// mutable borrow of the output can still call it.
fn scene_for<'frame>(
    intents: &ScrollIntents,
    buffer: &'frame mut Scene,
    frame: &'frame CommittedFrame,
    animation_now: Option<f64>,
) -> &'frame Scene {
    if !intents.overrides_any()
        && animation_now.is_none()
        && let Some(scene) = frame.scene()
    {
        return scene;
    }
    buffer.reset();
    frame.compose_into(buffer, &|slot| intents.offset_for(slot.node), animation_now);
    buffer
}

/// The scene to draw for a layered frame: its plan composed into `buffer` —
/// the raw steps at the in-flight offsets, each retained plane one textured
/// draw. The per-frame cost is the raw content plus one image draw per
/// plane; the scroller content moves without being re-encoded or
/// re-rasterized.
fn composite_scene<'frame>(
    intents: &ScrollIntents,
    buffer: &'frame mut Scene,
    frame: &CommittedFrame,
    plane_images: &[dom::vello::peniko::ImageData],
    animation_now: Option<f64>,
) -> &'frame Scene {
    buffer.reset();
    frame.composite_into(
        buffer,
        plane_images,
        &|slot| intents.offset_for(slot.node),
        animation_now,
    );
    buffer
}

/// Routes one input event against the published frame: the topmost
/// hit-testable element and its scroll slot, or nothing before the first
/// commit or outside every element.
///
/// The frame is baked unscrolled; `intents` supplies the offsets scrolls in
/// flight have already moved to, so a point lands on what the screen shows,
/// not on where the last commit left things.
fn route_published(
    frame: Option<&CommittedFrame>,
    intents: &ScrollIntents,
    event: &InputEvent,
    animation_now: Option<f64>,
) -> Option<HitTarget> {
    let finite = event.position.x.is_finite()
        && event.position.y.is_finite()
        && match event.kind {
            InputKind::Wheel { delta } => delta.x.is_finite() && delta.y.is_finite(),
            _ => true,
        };
    if !finite {
        debug_assert!(false, "host input events must be finite, got {event:?}");
        return None;
    }
    frame?.hit(
        event.position,
        &|slot| intents.offset_for(slot.node),
        animation_now,
    )
}

/// Everything one view is built from.
///
/// A view has exactly one entry MTS module, and everything that must be in
/// place before it runs is here beside it. All of it is applied to a document
/// no other thread has yet seen, so this type is what says a view boots once:
/// there is no method that starts a second entry, none that mounts a
/// stylesheet against a running page, and no installation that can collide
/// with a script batch.
#[derive(Clone)]
pub struct ViewSources {
    /// Font containers registered with the private text engine. Each is
    /// retained without copying its payload; one carrying no usable face
    /// registers nothing, which [`Self::default_font_family`] then reports.
    pub fonts: Vec<FontBlob>,
    /// Maps CSS `system-ui`, `sans-serif`, and `serif` to this family ahead of
    /// any platform fallbacks — primarily for embedders without system-font
    /// discovery, such as Wasm. A name neither [`Self::fonts`] nor the platform
    /// has fails construction.
    pub default_font_family: Option<String>,
    /// The store every frame reads its pixels from. It owns every decoded
    /// pixel the view draws; a view built without one paints no images.
    pub image_store: Option<Arc<dyn ImageStore>>,
    /// Author stylesheet URLs in cascade order: a sheet listed later wins ties
    /// against one listed earlier, exactly as later sheets do in a document.
    pub style_sheets: Vec<String>,
    /// The entry MTS module URL. The resolved form becomes the module
    /// specifier Bobcat's ESM boot module imports.
    pub entry: String,
}

impl ViewSources {
    /// The sources of a view that loads its entry module and nothing else.
    ///
    /// There is no `Default`: a view without an entry module is not a view.
    #[must_use]
    pub fn new(entry: impl Into<String>) -> Self {
        Self {
            fonts: Vec::new(),
            default_font_family: None,
            image_store: None,
            style_sheets: Vec::new(),
            entry: entry.into(),
        }
    }
}

impl fmt::Debug for ViewSources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ViewSources")
            .field("style_sheets", &self.style_sheets)
            .field("entry", &self.entry)
            .finish_non_exhaustive()
    }
}

/// Failure to load or start view content.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LynxViewError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Resource(#[from] crate::resource::ResourceError),
    #[error("script `{url}` is not valid UTF-8: {message}")]
    InvalidScriptEncoding { url: String, message: String },
    #[error("stylesheet `{url}` is not valid UTF-8: {message}")]
    InvalidStyleSheetEncoding { url: String, message: String },
    #[error("the image store could not load `{image_source}`: {message}")]
    Image {
        image_source: String,
        message: String,
    },
}

/// A view's entry MTS module: its source and the URL it registers under,
/// which is also the module specifier the boot module imports.
#[derive(Debug)]
struct EntryModule {
    source: String,
    url: String,
}

/// One change on its way to the document's owner — the Lynx main thread.
///
/// Only plain data crosses: node ids, geometry, a clock reading. The realm
/// and the document both stay where they are. The channel is ordered, so the
/// order decisions are made in is the order they apply in.
pub(crate) enum MainCommand {
    /// Deliver one routed event: the main thread validates the target is
    /// still live, computes the propagation path, and walks the realm's
    /// listeners.
    DispatchEvent {
        target: NodeId,
        name: Arc<str>,
        detail: Arc<str>,
    },
    /// New device metrics from the embedder.
    Resize {
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    },
    /// One animation tick: advance the timeline to `now` and commit what
    /// changed. `seq` is acknowledged through
    /// [`FrameHub::note_begin_frame_serviced`] so a synchronizing caller can
    /// wait for this specific tick.
    BeginFrame { now: f64, seq: u64 },
    /// The presenting side's compose offsets have consumed most of a slot's
    /// committed encode window: write the offsets the screen is showing back
    /// into the document and repaint, so the next commit re-centers the
    /// windows on them. This is the only channel a user scroll crosses on —
    /// between refills the presenting side's intents *are* the offsets, and
    /// nothing about a windowed scroll touches the main thread at all.
    Refill {
        offsets: Vec<(NodeId, Vector2D<f32>)>,
    },
    /// The installed store's answers changed; repaint with them.
    NoteImagesChanged,
    /// Run a closure against the document, for tests that observe main-thread
    /// state from the outside.
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
}

/// A running Lynx view: the shared element tree, input routing, frame
/// production, presentation, and the engine-owned script thread.
///
/// The document, element tree, script realm, and presentation pipeline are all
/// private implementation state. Embedders provide resources, a draw target,
/// and normalized OS events.
///
/// Generic over the embedder's [`Window`] capability types. A native surface
/// may borrow its window for the life of the engine; an owned browser canvas
/// target does not. The public windowless composition is
/// [`crate::OffscreenLynxView`].
///
/// Deliberately `!Send`: it lives on the thread the embedder calls it from.
pub struct LynxView<'window, W: Window = NoWindow> {
    /// The only `Sender` of the main thread's command channel, dropped first
    /// so this view's detached main thread wakes and releases its
    /// owner-thread-bound realm. It must never be cloned anywhere the script
    /// thread can reach.
    commands: mpsc::Sender<MainCommand>,
    /// A test-only view built without its main thread, so tests can hold
    /// the command receiver and observe the decision→command seam directly.
    #[cfg(test)]
    detached: bool,
    hub: Arc<FrameHub>,
    /// The store the paint walk reads, installed once at construction and
    /// held here too so async loaders reach it without the document.
    image_store: Arc<dyn dom::ImageStore>,
    viewport: Viewport,
    frame_size: FrameSize,
    messages: mpsc::Receiver<EngineEvent>,
    output: Output<'window>,
    /// The pending-frame bit and the host wakeup that carries it, shared
    /// with the Lynx main thread so a commit asks for its own frame.
    frames: FrameWakeup,
    /// The gesture recognizer: turns routed pointer sequences into Lynx's
    /// `tap`/`longpress` beside the raw pointer events, and decides the
    /// user-agent scroll — all against the published frame.
    gesture: GestureRouter,
    /// Which event names the realm has listeners for; written by the script
    /// thread's registration members, read when gestures resolve.
    listener_names: Arc<SharedListenerNames>,
    /// The animation timeline. Engine-owned and concrete: an embedder cannot
    /// name one, drive one, or observe this one. Sampled once per frame, on
    /// the presenting thread; the main thread receives the reading inside
    /// `BeginFrame`.
    clock: FrameClock,
    /// Between-commits scroll offsets, for consumption arbitration.
    scroll_intents: ScrollIntents,
    /// The compose key last drawn to the offscreen target.
    composed: Option<ComposeKey>,
    /// The buffer frames are composed into when in-flight scroll offsets
    /// override the committed ones; reused across frames.
    composed_scene: Scene,
    /// The commit id a refill was already requested for, so a long scroll
    /// asks once per commit instead of once per event.
    refill_requested_for: Option<u64>,
    /// `BeginFrame` sequence numbers, acknowledged through the hub.
    begin_frames_sent: u64,
    /// The window type this view attaches, which no field holds — the
    /// `fn() -> W` imposes nothing on `W` itself.
    window: PhantomData<fn() -> W>,
    thread_bound: PhantomData<Rc<()>>,
}

/// The offscreen composition of [`LynxView`].
pub type OffscreenLynxView = LynxView<'static, NoWindow>;

impl<W: Window> fmt::Debug for LynxView<'_, W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

impl<'window, W: Window> LynxView<'window, W> {
    /// Loads a view's sources and returns it already running.
    ///
    /// This is the whole of a view's boot: everything named by URL is fetched
    /// through `resource_fetcher`, all of `sources` is applied to a document
    /// no other thread has seen, and the Lynx main thread is started over that
    /// document with the one entry module. A source that will not load, or a
    /// thread that will not start, produces an error instead of a half-built
    /// view. `sources` is consumed and the fetcher is borrowed only for this
    /// call. Entry-module completion is reported through
    /// [`EngineEvent::ScriptFinished`] or [`EngineEvent::ScriptRunError`].
    ///
    /// The animation timeline is the engine's own: a host neither names one
    /// nor drives it. Frames are paced by the swap chain, and the clock is
    /// sampled once per frame, after the acquire that waits on vsync.
    pub async fn new(
        config: PageConfig,
        resource_fetcher: &dyn ResourceFetcher,
        event_requester: Arc<dyn EventRequester>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        sources: ViewSources,
    ) -> Result<Self, LynxViewError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let mut document = new_document(viewport, config);
        for font in sources.fonts {
            document.register_fonts(font);
        }
        if let Some(family) = sources.default_font_family
            && !document.set_default_font_family(&family)
        {
            return Err(EngineError::UnknownFontFamily(family).into());
        }
        if let Some(store) = sources.image_store {
            document.set_image_store(store);
        }

        // One namespace per boot separates concurrently constructed views; the
        // sequence within it is one per stylesheet plus one for the entry.
        let mut requests = RequestId {
            namespace: NEXT_REQUEST_NAMESPACE.fetch_add(1, Ordering::Relaxed),
            sequence: 0,
        };
        for url in &sources.style_sheets {
            mount_style_sheet(resource_fetcher, &mut requests, url, &mut document).await?;
        }
        let entry = fetch_entry(resource_fetcher, &mut requests, &sources.entry).await?;
        Ok(Self::start(
            document,
            viewport,
            frame_size,
            event_requester,
            entry,
        )?)
    }

    /// Hands the finished document to its Lynx main thread — the document's
    /// owner for the rest of its life.
    ///
    /// The half of [`Self::new`] that touches no IO, so the crate's own tests
    /// can start a view without a resource provider.
    fn start(
        document: LynxDocument,
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<dyn EventRequester>,
        entry: EntryModule,
    ) -> Result<Self, EngineError> {
        // Captured before the document leaves for its owner thread, so async
        // loaders reach the store without it.
        let image_store = Arc::clone(document.image_store());
        let (view, receiver, events) =
            Self::with_channel(image_store, viewport, frame_size, event_requester);
        spawn_main_thread(
            document,
            entry,
            receiver,
            Arc::clone(&view.hub),
            Arc::clone(&view.listener_names),
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

    /// The view minus its main thread: every field constructed, the command
    /// receiver and the thread's event sender handed back instead of served.
    fn with_channel(
        image_store: Arc<dyn dom::ImageStore>,
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<dyn EventRequester>,
    ) -> (Self, mpsc::Receiver<MainCommand>, EngineEventSender) {
        let (message_sender, messages) = mpsc::channel();
        let (commands, command_receiver) = mpsc::channel();
        let frames = FrameWakeup {
            pending: Arc::new(AtomicBool::new(false)),
            requester: Arc::clone(&event_requester),
        };
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
            listener_names: Arc::new(SharedListenerNames::default()),
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
        (view, command_receiver, events)
    }

    /// Runs `probe` against the main-thread document over the command
    /// channel, waiting for the answer. `None` on a detached test view or
    /// after the main thread is gone.
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

    /// The latest published frame, for tests that assert on commits.
    #[cfg(test)]
    pub(crate) fn published_frame(&self) -> Option<Arc<CommittedFrame>> {
        self.hub.latest()
    }

    /// Whether the engine owes the timeline another frame: the last produced
    /// frame left an animation running, or a gesture deadline is armed and
    /// waiting on the clock.
    ///
    /// This is the one continuation signal an offscreen embedder has — that
    /// output draws on the host's own ticks, so a host that idles its tick
    /// loop must keep ticking while this reports `true` or an armed
    /// long-press never resolves.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.hub
            .latest()
            .is_some_and(|frame| frame.animations_active())
            || self.gesture.needs_frame()
    }

    /// Loads one image through the installed store and repaints with it.
    ///
    /// The store is reached without the document; the repaint is one queued
    /// command to the document's owner thread, so a load can never collide
    /// with a script batch.
    pub async fn load_image(&mut self, source: &str) -> Result<(), LynxViewError> {
        let store = self.image_store();
        store
            .get(source)
            .await
            .map_err(|error| LynxViewError::Image {
                image_source: source.to_owned(),
                message: error.to_string(),
            })?;
        self.note_images_changed();
        Ok(())
    }

    /// Asks the installed store to start loading `source` without waiting for
    /// it, discarding both the pixels and any failure.
    ///
    /// The pixels reach the screen on the first frame after they land only if
    /// something else invalidates the scene; a prefetch is a warm-up, not a
    /// load. Use [`Self::load_image`] for an image the next frame must draw.
    pub fn prefetch_image(&self, source: &str) {
        self.image_store().prefetch(source);
    }

    /// The current physical render-target size in device pixels.
    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// A handle on the installed store, for a caller that must reach it
    /// without holding the document across an await.
    fn image_store(&self) -> Arc<dyn dom::ImageStore> {
        Arc::clone(&self.image_store)
    }

    /// Rebuilds the next frame's scene because the installed store's answers
    /// changed, and asks for that frame.
    fn note_images_changed(&mut self) {
        let _ = self.commands.send(MainCommand::NoteImagesChanged);
        self.refresh();
    }

    /// Routes one host input event on the presenting side: hit test and
    /// gesture recognition against the published frame, scroll and dispatch
    /// decisions queued as commands at once. Nothing buffers and nothing
    /// blocks.
    pub fn dispatch_input(&mut self, event: InputEvent) {
        let at = self.clock.now_seconds();
        let published = self.hub.latest();
        if let Some(frame) = &published {
            self.scroll_intents.rebase(frame);
        }
        let generation = self.scroll_intents.generation;
        let frame = published.as_deref();
        let animation_now = frame.and_then(|frame| frame.has_live_curves().then_some(at));
        let target = route_published(frame, &self.scroll_intents, &event, animation_now);
        let mut decisions = Vec::new();
        {
            let host = FrameRouterHost {
                frame,
                listener_names: &self.listener_names,
            };
            self.gesture
                .on_input(&event, target, at, &host, &mut decisions);
        }
        self.execute_decisions(&mut decisions, published.as_ref());
        if let Some(frame) = &published {
            self.maybe_request_refill(frame);
        }
        // A consumed scroll changes what composition shows; an armed
        // long-press deadline needs the frame clock even when nothing is
        // dirty. Either way the next frame is this side's to produce.
        if self.gesture.needs_frame() || self.scroll_intents.generation != generation {
            self.refresh();
        }
    }

    /// Writes the offsets the screen is showing back to the main thread
    /// when in-flight offsets have consumed most of a slot's encode window —
    /// once per committed frame, because only the next commit re-centers
    /// the windows.
    fn maybe_request_refill(&mut self, frame: &CommittedFrame) {
        if self.refill_requested_for == Some(frame.commit_id())
            || !self.scroll_intents.refill_due(frame)
        {
            return;
        }
        self.refill_requested_for = Some(frame.commit_id());
        let _ = self.commands.send(MainCommand::Refill {
            offsets: self.scroll_intents.writeback(),
        });
    }

    /// Executes the router's decisions in order — which is the delivery
    /// order, because the command channel is ordered.
    fn execute_decisions(
        &mut self,
        decisions: &mut Vec<InputDecision>,
        published: Option<&Arc<CommittedFrame>>,
    ) {
        let Self {
            commands,
            gesture,
            scroll_intents,
            listener_names,
            ..
        } = self;
        for decision in decisions.drain(..) {
            match decision {
                InputDecision::Scroll {
                    pointer,
                    from,
                    delta,
                } => {
                    let consumed =
                        published.is_some_and(|frame| scroll_intents.chain(frame, from, delta));
                    if consumed && let Some(pointer) = pointer {
                        gesture.note_scroll_consumed(pointer);
                    }
                }
                InputDecision::Emit(event) => {
                    // Asked before anything is built: an event no listener
                    // registered for costs one lookup instead of a
                    // cross-thread wakeup. The table is what the realm has
                    // registered *so far*; the staleness window is one
                    // routing pass wide (see docs/tracking/dom-events.md).
                    if !listener_names.contains(event.name) {
                        continue;
                    }
                    // Liveness is the main thread's to check, where the
                    // document is; a freed target resolves to nothing there.
                    let _ = commands.send(MainCommand::DispatchEvent {
                        target: event.target,
                        name: Arc::from(event.name),
                        detail: Arc::from(emit_detail(&event)),
                    });
                }
            }
        }
    }

    /// Resolves gesture deadlines against the frame clock — the per-frame
    /// half of the router, beside the per-event half in
    /// [`Self::dispatch_input`]. While a deadline is armed the router's
    /// [`GestureRouter::needs_frame`] keeps frames coming, the same
    /// continuation contract running animations use.
    fn service_gesture_clock(&mut self, now: f64) {
        let published = self.hub.latest();
        let mut decisions = Vec::new();
        {
            let host = FrameRouterHost {
                frame: published.as_deref(),
                listener_names: &self.listener_names,
            };
            self.gesture.on_tick(now, &host, &mut decisions);
        }
        self.execute_decisions(&mut decisions, published.as_ref());
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
        let _ = self.commands.send(MainCommand::Resize {
            width,
            height,
            device_pixel_ratio,
        });
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.refresh();
        Ok(())
    }

    /// Asks for a frame: records it and wakes the host event loop, whose
    /// next turn draws it. No OS frame callback is involved.
    pub fn refresh(&self) {
        self.frames.request();
    }

    /// Drains lifecycle messages from engine-owned threads.
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        self.messages.try_iter().collect()
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
    /// Native embedders may use this internally when they already have an
    /// async initialization context; the public synchronous convenience is
    /// [`crate::LynxView::attach_window`]. Browser embedders attach an owned
    /// canvas through [`LynxView::attach_target`] instead.
    #[cfg(not(target_arch = "wasm32"))]
    async fn attach_window_async(
        &mut self,
        window: &'window W,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        self.attach_target(window.target(), size).await
    }

    /// Attaches an already-owned surface target.
    ///
    /// This is the browser-friendly form: `SurfaceTarget::Canvas` owns a
    /// JavaScript canvas reference, so the Wasm wrapper does not need a
    /// self-referential Rust struct merely to keep a `Window` borrow alive.
    pub async fn attach_target(
        &mut self,
        target: impl Into<WindowTarget<'window>>,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        let graphics = WindowGraphics::new(target, size).await?;
        self.output = Output::Window(Box::new(graphics));
        self.refresh();
        Ok(())
    }

    /// Sends one `BeginFrame` when something animating still needs the main
    /// thread (or `always`), returning the sequence number sent.
    fn begin_frame(&mut self, now: f64, always: bool) -> Option<u64> {
        // A detached test view has nobody serving; a queued `BeginFrame`
        // would only stall a synchronizing caller.
        #[cfg(test)]
        if self.detached {
            return None;
        }
        // An exported curve animates on this side; the main thread is asked
        // for a tick only for what could not export, or once a curve has
        // run past its domain and needs its finish restyle.
        let main_ticks_due = self
            .hub
            .latest()
            .is_some_and(|frame| frame.needs_main_ticks() || frame.animation_boundary_passed(now));
        if !main_ticks_due && !always {
            return None;
        }
        self.begin_frames_sent += 1;
        let seq = self.begin_frames_sent;
        self.commands
            .send(MainCommand::BeginFrame { now, seq })
            .ok()
            .map(|()| seq)
    }

    /// Draws the frame the engine asked for, if it asked for one: acquire,
    /// compose the latest published frame, present. The document is never
    /// touched — the main thread is asked for the next animation tick, and
    /// what is already published is composed.
    ///
    /// The embedder calls this on every host wakeup, beside [`Self::pump`],
    /// and nothing else gates a frame: a commit, a consumed scroll, or an
    /// armed deadline records one and wakes the loop, and that wakeup is the
    /// one that draws it. A wakeup carrying no frame costs one bit read.
    pub fn draw(&mut self) -> Result<(), EngineError> {
        if !matches!(self.output, Output::Window(_)) {
            return Ok(());
        }
        // Taken before any of the frame's work: a commit that lands while
        // this frame composes leaves the bit set for the next one.
        if !self.frames.take() {
            return Ok(());
        }
        let frames = self.frames.clone();
        let size = self.frame_size;
        // Take the swap-chain image before doing any of the frame's work.
        // `AutoVsync` makes this the call that waits, and everything after it
        // then belongs to the frame that image will display — including the
        // clock reading, which would otherwise be a whole swap-chain pipeline
        // stale by the time the pixels it produced reach the screen.
        let acquired = {
            let Output::Window(graphics) = &mut self.output else {
                unreachable!("the window output was just checked");
            };
            match graphics.acquire(size)? {
                FrameAcquisition::Ready(acquired) => acquired,
                FrameAcquisition::Retry => {
                    frames.request();
                    return Ok(());
                }
            }
        };
        // The frame's one instant. Gesture deadlines resolve against the same
        // reading the committer's animations will, so nothing in a frame
        // disagrees about when the frame is.
        let now = self.clock.now_seconds();
        self.service_gesture_clock(now);
        let _ = self.begin_frame(now, false);
        let latest = self.hub.latest();
        if let Some(frame) = &latest {
            self.scroll_intents.rebase(frame);
            self.maybe_request_refill(frame);
        }
        let animating = latest
            .as_ref()
            .is_some_and(|frame| frame.animations_active());
        let key = latest
            .as_ref()
            .map(|frame| (frame.commit_id(), self.scroll_intents.generation));
        let animation_now = latest
            .as_ref()
            .and_then(|frame| frame.has_live_curves().then_some(now));
        let Output::Window(graphics) = &mut self.output else {
            unreachable!("the window output was just checked");
        };
        if let (Some(frame), Some(key)) = (&latest, key)
            && (animation_now.is_some() || graphics.needs_paint(key, size))
        {
            let scene = if frame.composite_plan().is_some() {
                graphics.prepare_planes(frame)?;
                composite_scene(
                    &self.scroll_intents,
                    &mut self.composed_scene,
                    frame,
                    graphics.plane_images(),
                    animation_now,
                )
            } else {
                scene_for(
                    &self.scroll_intents,
                    &mut self.composed_scene,
                    frame,
                    animation_now,
                )
            };
            graphics.render_to_target(scene, size, key)?;
        }
        if animating || self.gesture.needs_frame() {
            frames.request();
        }
        // Nothing rendered at this size means nothing was ever published for
        // it: the acquired image goes back unpresented and nothing is asked
        // for, because the commit that ends the wait asks for its own frame.
        // Asking here instead would spin against a committer that may never
        // publish — one that failed its boot never will.
        if graphics.rendered_at(size) {
            graphics.present(acquired);
        }
        Ok(())
    }

    /// Captures the current frame as pixels — synchronously, from whichever
    /// target is attached: what is published is what the window is showing.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        let size = self.frame_size;
        let now = self.clock.now_seconds();
        let latest = self.hub.latest();
        let key = latest
            .as_ref()
            .map(|frame| (frame.commit_id(), self.scroll_intents.generation));
        let animation_now = latest
            .as_ref()
            .and_then(|frame| frame.has_live_curves().then_some(now));
        match &mut self.output {
            Output::None => Err(EngineError::NoDrawTarget),
            Output::Offscreen(gpu) => {
                if let (Some(frame), Some(key)) = (&latest, key)
                    && (self.composed != Some(key) || animation_now.is_some())
                {
                    let scene = if frame.composite_plan().is_some() {
                        gpu.prepare_planes(frame)
                            .map_err(|error| EngineError::Gpu(error.to_string()))?;
                        composite_scene(
                            &self.scroll_intents,
                            &mut self.composed_scene,
                            frame,
                            gpu.plane_images(),
                            animation_now,
                        )
                    } else {
                        scene_for(
                            &self.scroll_intents,
                            &mut self.composed_scene,
                            frame,
                            animation_now,
                        )
                    };
                    gpu.render_frame(scene, size.width, size.height, Color::WHITE)
                        .map_err(|error| EngineError::Gpu(error.to_string()))?;
                    self.composed = Some(key);
                }
                let pixels = gpu
                    .read_pixels()
                    .map_err(|error| EngineError::Gpu(error.to_string()))?;
                Ok(Screenshot { size, pixels })
            }
            Output::Window(graphics) => {
                if let (Some(frame), Some(key)) = (&latest, key)
                    && (animation_now.is_some() || graphics.needs_paint(key, size))
                {
                    let scene = if frame.composite_plan().is_some() {
                        graphics.prepare_planes(frame)?;
                        composite_scene(
                            &self.scroll_intents,
                            &mut self.composed_scene,
                            frame,
                            graphics.plane_images(),
                            animation_now,
                        )
                    } else {
                        scene_for(
                            &self.scroll_intents,
                            &mut self.composed_scene,
                            frame,
                            animation_now,
                        )
                    };
                    graphics.render_to_target(scene, size, key)?;
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
}

/// Starts the one Lynx main thread a view has, handing it the document it
/// will own for the rest of its life.
///
/// The thread creates the `QuickJS` realm and the main-thread runtime over
/// the document, registers `entry` at its resolved URL, runs Bobcat's ESM
/// boot module, and then serves the command channel for as long as the view
/// holds its sender. A failed boot reports and ends the thread — the view is
/// over, exactly as when a source failed to load.
fn spawn_main_thread(
    document: LynxDocument,
    entry: EntryModule,
    commands: mpsc::Receiver<MainCommand>,
    hub: Arc<FrameHub>,
    listener_names: Arc<SharedListenerNames>,
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
                let mut runtime = crate::runtime::MainThreadRuntime::new(
                    document,
                    listener_names,
                    publish_hub,
                    move || wake_frames.request(),
                )
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
                // The document's own pipeline is let-it-crash: a style,
                // layout, or paint assertion ends this thread. Report it
                // first — the embedder's pump loop is its fatal-error
                // channel, and a silent exit would freeze the view on the
                // last published frame with no explanation.
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

/// Serves the command channel for as long as the engine holds its sender.
///
/// The realm outlives its entry module; the channel closing is the only
/// shutdown signal this thread needs or gets. Commands are applied in
/// order; each round drains what is already queued and ends with one commit
/// when anything went stale, published before any `BeginFrame` in the round
/// is acknowledged. The round needs no command cap: every producer is
/// frame- or event-rate-bounded — `BeginFrame` once per frame, input as the
/// OS delivers it (pointer moves coalesced on the presenting side), and
/// scrolls not at all, since a windowed scroll never crosses — so the queue
/// empties faster than it fills and the drain terminates on its own.
fn serve_main_commands(
    mut runtime: crate::runtime::MainThreadRuntime,
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
    runtime: &mut crate::runtime::MainThreadRuntime,
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
            // A panicking listener must not take the realm with it: the next
            // event still has to arrive.
            let delivered = catch_unwind(AssertUnwindSafe(|| {
                runtime.dispatch_event(target, &name, &detail)
            }));
            // A panic is already the crate's unspecified-state contract, and
            // the unwind carries no `ScriptError` to report; the realm
            // survives it, which is what the `catch_unwind` is for.
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
        // A panic ends this runtime rather than reporting an ordinary script
        // exception. The abort hook cannot identify the active VM boundary,
        // so `Execute` denotes owner-thread execution generally.
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

/// Fetches one author stylesheet and mounts it, in the form its provider had.
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
        StyleSheetPayload::Text(bytes) => match str::from_utf8(&bytes) {
            Ok(css) => crate::style::add_style_sheet_text(document, css),
            Err(error) => {
                return Err(LynxViewError::InvalidStyleSheetEncoding {
                    url: source_name,
                    message: error.to_string(),
                });
            }
        },
    }
    Ok(())
}

/// Fetches the entry MTS module. Last, so a stylesheet that will not load
/// leaves no thread running.
async fn fetch_entry(
    fetcher: &dyn ResourceFetcher,
    requests: &mut RequestId,
    url: &str,
) -> Result<EntryModule, LynxViewError> {
    let (request, url) = resolve_for_fetch(fetcher, requests, url).await?;
    let response = fetcher.fetch_resource(request).await?;
    match str::from_utf8(&response.bytes) {
        Ok(source) => Ok(EntryModule {
            source: source.to_owned(),
            url,
        }),
        Err(error) => Err(LynxViewError::InvalidScriptEncoding {
            url,
            message: error.to_string(),
        }),
    }
}

/// Resolves a locator and builds the request for it — the prologue every
/// URL-shaped source load shares. Returns the request and the resolved URL,
/// which is both the entry's module specifier and the name any encoding
/// failure is reported under.
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

impl OffscreenLynxView {
    /// Attaches an offscreen GPU target.
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        let gpu = Headless::new().map_err(|error| EngineError::Gpu(error.to_string()))?;
        self.output = Output::Offscreen(Box::new(gpu));
        self.composed = None;
        Ok(())
    }

    /// Renders one frame to the offscreen target if a newer commit was
    /// published (or unconditionally with `force`), returning whether a frame
    /// was submitted.
    ///
    /// This is the embedder's synthetic vsync, and deliberately also its one
    /// synchronization point: the main thread is sent a `BeginFrame` and
    /// waited on, so the composed pixels deterministically include every
    /// command queued before this call. A windowed frame never waits; this
    /// offscreen path trades that law for reproducibility.
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        if !matches!(self.output, Output::Offscreen(_)) {
            return Err(EngineError::NoDrawTarget);
        }
        let now = self.clock.now_seconds();
        self.service_gesture_clock(now);
        if let Some(seq) = self.begin_frame(now, true) {
            let _ = self.hub.wait_begin_frame(seq, BEGIN_FRAME_TIMEOUT);
        }
        let Some(frame) = self.hub.latest() else {
            return Ok(false);
        };
        self.scroll_intents.rebase(&frame);
        self.maybe_request_refill(&frame);
        let key = (frame.commit_id(), self.scroll_intents.generation);
        let animation_now = frame.has_live_curves().then_some(now);
        if self.composed == Some(key) && !force && animation_now.is_none() {
            return Ok(false);
        }
        let Output::Offscreen(gpu) = &mut self.output else {
            unreachable!("the offscreen output was just checked");
        };
        let scene = if frame.composite_plan().is_some() {
            gpu.prepare_planes(&frame)
                .map_err(|error| EngineError::Gpu(error.to_string()))?;
            composite_scene(
                &self.scroll_intents,
                &mut self.composed_scene,
                &frame,
                gpu.plane_images(),
                animation_now,
            )
        } else {
            scene_for(
                &self.scroll_intents,
                &mut self.composed_scene,
                &frame,
                animation_now,
            )
        };
        gpu.render_frame(
            scene,
            self.frame_size.width,
            self.frame_size.height,
            Color::WHITE,
        )
        .map_err(|error| EngineError::Gpu(error.to_string()))?;
        gpu.wait_idle()
            .map_err(|error| EngineError::Gpu(error.to_string()))?;
        self.composed = Some(key);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use super::{MainCommand, frame_size};
    use crate::tree::{LynxDocument, PageConfig, Viewport, new_document};

    /// A phone-shaped document, ready for a main thread to be started over it.
    fn document() -> LynxDocument {
        new_document(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    /// Starts a view over `document` and `entry`, the IO-free half of
    /// construction.
    fn view_over(
        events: Arc<dyn super::EventRequester>,
        document: LynxDocument,
        entry: &str,
    ) -> super::OffscreenLynxView {
        super::LynxView::start(
            document,
            Viewport::new(393.0, 727.0),
            frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
            events,
            super::EntryModule {
                source: entry.to_owned(),
                url: "app:///main.js".to_owned(),
            },
        )
        .expect("the test view starts")
    }

    /// A view with every seam built but no main thread: the command receiver
    /// is handed back so a test can observe the decision→command seam
    /// directly. Probes answer `None` and `BeginFrame`s are withheld —
    /// nobody would ever service them.
    fn detached() -> (super::OffscreenLynxView, mpsc::Receiver<MainCommand>) {
        let document = document();
        let store = Arc::clone(document.image_store());
        let (view, receiver, _events) = super::LynxView::with_channel(
            store,
            Viewport::new(393.0, 727.0),
            frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
            Arc::new(|| {}),
        );
        (view, receiver)
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

    /// The store a view is built over is the one the paint walk reads, and
    /// the pixels reach it without a copy: the buffer identity that comes
    /// back out of the main-thread document is the one that went in.
    #[test]
    fn the_installed_image_store_is_the_one_the_document_reads() {
        let mut document = document();
        let images = Arc::new(flashbulb::TestImages::new());
        let pixels = flashbulb::rgba8(1, 1, vec![1, 2, 3, 255]);
        let pixel_id = pixels.data.id();
        images.insert("app:///pixel.png", pixels);
        document.set_image_store(Arc::clone(&images) as Arc<dyn dom::ImageStore>);

        let mut view = view_over(
            Arc::new(|| {}),
            document,
            "globalThis.renderPage = function () { __CreatePage('card', 0); };",
        );
        let (hit, miss) = view
            .probe_document(move |tree| {
                (
                    tree.image_store()
                        .peek("app:///pixel.png")
                        .map(|image| image.data.id()),
                    tree.image_store().peek("app:///missing.png").is_none(),
                )
            })
            .expect("the main thread answers probes");
        assert_eq!(hit, Some(pixel_id));
        assert!(miss);
    }

    /// An emit decision costs one name-table lookup and stops there unless a
    /// listener exists; when it crosses, it crosses as plain data — the
    /// target id, not a path. Liveness is the main thread's to check at
    /// delivery.
    #[test]
    fn an_emit_decision_crosses_only_when_a_listener_wants_it() {
        use crate::gesture::{EmitEvent, InputDecision, TAP_EVENT};

        let (mut view, commands) = detached();
        // The permanent page element's packed handle, as script would name it.
        let target = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
        let emit = |view: &mut super::OffscreenLynxView| {
            let mut decisions = vec![InputDecision::Emit(EmitEvent {
                name: TAP_EVENT,
                target,
                position: dom::Point2D::new(1.0, 1.0),
                wheel: None,
            })];
            view.execute_decisions(&mut decisions, None);
            assert!(decisions.is_empty(), "the queue is always drained");
        };

        emit(&mut view);
        assert!(
            commands.try_recv().is_err(),
            "an empty listener table sends nothing"
        );

        view.listener_names.note_enabled("pointerup");
        emit(&mut view);
        assert!(
            commands.try_recv().is_err(),
            "a listener on another name sends nothing"
        );

        view.listener_names.note_enabled(TAP_EVENT);
        emit(&mut view);
        let command = commands.try_recv().expect("the listened-for name crosses");
        let MainCommand::DispatchEvent {
            name, target: sent, ..
        } = command
        else {
            panic!("an emit decision becomes a dispatch command");
        };
        assert_eq!(name.as_ref(), TAP_EVENT);
        assert_eq!(sent, target);

        // And the count is a count: the last removal is what closes the name.
        view.listener_names.note_disabled(TAP_EVENT);
        emit(&mut view);
        assert!(
            commands.try_recv().is_err(),
            "the removed registration stops the crossing"
        );
    }

    /// A scroll decision crosses nothing: it lands in the presenting side's
    /// intents, which are the offsets composition shows, and the main
    /// thread hears about scrolling only when a refill writes offsets back.
    /// With no published frame there is no geometry to consume against, so
    /// the decision evaporates entirely.
    #[test]
    fn a_scroll_decision_sends_no_command() {
        use crate::gesture::InputDecision;

        let (mut view, commands) = detached();
        let node = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
        let mut decisions = vec![InputDecision::Scroll {
            pointer: None,
            from: node,
            delta: dom::Vector2D::new(0.0, 5.0),
        }];
        view.execute_decisions(&mut decisions, None);
        assert!(
            commands.try_recv().is_err(),
            "a windowed scroll never crosses the command channel"
        );
        assert!(view.scroll_intents.offsets.is_empty());
    }

    /// Boot's final flush is a commit: by the time `ScriptFinished` is
    /// pumped, a frame is published and the document — owned by the main
    /// thread — answers probes.
    #[test]
    fn a_booted_view_commits_and_publishes() {
        use std::time::{Duration, Instant};

        use super::EngineEvent;

        let (wake_sender, wake_receiver) = mpsc::channel();
        let mut view = view_over(
            Arc::new(move || {
                let _ = wake_sender.send(());
            }),
            document(),
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              __AppendElement(page, __CreateView(0));
              __FlushElementTree();
              __AppendElement(page, __CreateView(0));
            };
            ",
        );

        // One wakeup carries either kind of engine work: the boot commit's
        // frame or a lifecycle event. The law under test is the ordering —
        // whichever wakeup carries the event, `pump` observes it right then,
        // with nothing polled for and nothing slept on.
        let deadline = Instant::now() + Duration::from_secs(5);
        let finished = loop {
            wake_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("script completion must wake the host event loop");
            if let Some(event) = view.pump().into_iter().find(|event| {
                matches!(
                    event,
                    EngineEvent::ScriptFinished | EngineEvent::ScriptRunError(_)
                )
            }) {
                break event;
            }
            assert!(
                Instant::now() < deadline,
                "no wakeup ever carried the script-completion event"
            );
        };
        assert!(matches!(finished, EngineEvent::ScriptFinished));

        let frame = view
            .published_frame()
            .expect("the boot's flush published a committed frame");
        assert!(frame.commit_id() > 0);

        let (views, connected, laid_out) = view
            .probe_document(|tree| {
                let page = tree.document_element().id();
                let views = tree
                    .get(page)
                    .expect("the page is live")
                    .child_ids()
                    .to_vec();
                let connected = views.iter().all(|&view| tree.is_connected(view));
                (views.len(), connected, tree.rounded_layout(page).is_some())
            })
            .expect("the main thread answers probes");
        assert_eq!(views, 2, "the boot script appends two views");
        assert!(connected, "both views are attached");
        assert!(laid_out, "the boot's final flush laid the page out");
    }
}

#[cfg(test)]
mod event_loop_tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use dom::Point2D;
    use dom::input::{InputEvent, PointerKind, PointerPhase};

    use super::OffscreenLynxView;

    /// The handle a packed id names, the way script spells one.
    fn node_id(bits: u64) -> dom::NodeId {
        dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
    }

    /// Boots a script and waits for it to finish, leaving the main thread
    /// parked on its command channel with the boot's frame published.
    fn booted(source: &str) -> OffscreenLynxView {
        let document = crate::tree::new_document(
            crate::tree::Viewport::new(393.0, 727.0),
            crate::tree::PageConfig::default(),
        );
        let mut engine = super::LynxView::start(
            document,
            crate::tree::Viewport::new(393.0, 727.0),
            super::frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
            Arc::new(|| {}),
            super::EntryModule {
                source: source.to_owned(),
                url: "app:///main.js".to_owned(),
            },
        )
        .expect("the test view starts");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if engine
                .pump()
                .into_iter()
                .any(|event| matches!(event, crate::EngineEvent::ScriptFinished))
            {
                assert!(
                    engine.published_frame().is_some(),
                    "boot's flush publishes before ScriptFinished is pumped"
                );
                return engine;
            }
            assert!(Instant::now() < deadline, "the entry module did not finish");
            std::thread::yield_now();
        }
    }

    /// One attribute of one node, read on the main thread through a probe.
    fn attribute_of(
        engine: &mut OffscreenLynxView,
        node: u64,
        name: &'static str,
    ) -> Option<String> {
        engine
            .probe_document(move |tree| {
                tree.get(node_id(node))
                    .and_then(|live| live.attribute(name).map(str::to_owned))
            })
            .flatten()
    }

    /// The whole loop: input arrives on this thread, is routed against the
    /// published frame and decided here, and delivered to a listener on the
    /// thread that owns the realm and the document.
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
                // Observable from the presenting side, and proof delivery ran
                // where the document is.
                __SetAttribute(view, 'seen', event.type + ':' + event.detail.x);
              }, {});
              __FlushElementTree();
            };
            ",
        );

        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(10.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));

        // Delivery is asynchronous by construction: this thread queued a
        // command and moved on.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let seen = attribute_of(&mut engine, 3, "seen");
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

    /// A listener that throws must not take the loop with it: the failure is
    /// reported rather than swallowed, and the next event still delivers.
    #[test]
    fn a_throwing_listener_keeps_the_loop_alive_and_is_reported() {
        let mut engine = booted(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              globalThis.count = 0;
              __SetInlineStyles(view, 'width:200px;height:200px');
              __AddEventListener(view, 'pointerdown', () => {
                count += 1;
                __SetAttribute(view, 'seen', String(count));
                throw new Error('a listener may fail');
              }, {});
              __FlushElementTree();
            };
            ",
        );

        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(10.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));

        let mut reported = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            reported |= engine.pump().into_iter().any(|event| {
                matches!(event, crate::EngineEvent::ListenerFailed(error)
                    if error.message.contains("a listener may fail"))
            });
            let seen = attribute_of(&mut engine, 3, "seen");
            if seen.as_deref() == Some("1") && reported {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "listener ran: {seen:?}, failure reported: {reported}"
            );
            std::thread::yield_now();
        }

        // And the loop still works: a second event routes and is delivered.
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(10.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if attribute_of(&mut engine, 3, "seen").as_deref() == Some("2") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "a thrown listener must not wedge delivery"
            );
            std::thread::yield_now();
        }
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

        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(10.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));

        std::thread::sleep(Duration::from_millis(50));
        assert!(attribute_of(&mut engine, 3, "seen").is_none());
    }

    /// The gesture suite's page: one 200x200 view whose listeners append
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
    fn wait_for_log(engine: &mut OffscreenLynxView, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let log = attribute_of(engine, 3, "log");
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
        let mut engine = booted(&gesture_page(true));
        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        engine.clock.pin(0.6);
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
        let mut engine = booted(&gesture_page(false));
        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        engine.clock.pin(0.6);
        engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
        wait_for_log(&mut engine, "tap:10");
    }

    /// Input processed after the deadline resolves the deadline first: the
    /// decision order is the delivery order on the ordered channel, so
    /// `longpress` precedes the release that follows it.
    #[test]
    fn a_release_after_the_deadline_delivers_longpress_before_the_release() {
        let mut engine = booted(&gesture_page(true));
        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        engine.clock.pin(0.6);
        engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
        engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
        engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
        wait_for_log(&mut engine, "longpress:10,tap:30");
    }

    /// A scrollable page: the 200x200 view scrolls a 1000px-tall child, and
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

    fn scroll_offset_of(engine: &mut OffscreenLynxView, node: u64) -> dom::Vector2D<f32> {
        engine
            .probe_document(move |tree| tree.scroll_offset(node_id(node)))
            .expect("the main thread answers probes")
    }

    /// A drag the user-agent scroll consumed is the claim that suppresses
    /// `tap` — end to end: recognition against the published scroll-slot
    /// table, consumption arbitrated against published bounds, the scroll
    /// applied authoritatively on the main thread. The drag travels 30px:
    /// past the 8px drag slop so it scrolls, inside the 50px tap slop so the
    /// claim is the only suppressor. The fence tap at another x pins that
    /// the suppressed one never crossed the channel.
    #[test]
    fn a_scroll_consuming_drag_suppresses_the_tap() {
        let mut engine = booted(SCROLLING_GESTURE_PAGE);
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

        // The router's scroll decision landed in the intents: 30px of
        // travel minus the 8px drag slop moved the scroller 22px. The
        // document never hears about a windowed scroll.
        let offset = engine
            .scroll_intents
            .offset_for(node_id(3))
            .expect("the drag scrolled the view");
        assert!(
            (offset.y - 22.0).abs() < 0.5,
            "the drag scrolled the view, got {offset:?}"
        );
        assert_eq!(
            scroll_offset_of(&mut engine, 3),
            dom::Vector2D::zero(),
            "a windowed scroll leaves the document untouched"
        );
    }

    /// A wheel over scrollable content scrolls it (the router's decision,
    /// landing in the intents) and dispatches `wheel` with its delta in
    /// the detail — in that order.
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
        engine.dispatch_input(InputEvent::wheel(
            Point2D::new(100.0, 100.0),
            dom::Vector2D::new(0.0, 30.0),
        ));
        wait_for_log(&mut engine, "wheel:30");
        let offset = engine
            .scroll_intents
            .offset_for(node_id(3))
            .expect("the wheel scrolled the view");
        assert!(
            (offset.y - 30.0).abs() < 0.5,
            "the wheel scrolled the view, got {offset:?}"
        );
    }

    /// A stationary hold produces no further input, so only the frame half
    /// — `service_gesture_clock` plus the `needs_frame` continuation — can
    /// resolve it. This drives that half exactly as `draw`/`tick`
    /// do, without needing a GPU output.
    #[test]
    fn a_stationary_hold_longpresses_on_the_frame_clock() {
        let mut engine = booted(&gesture_page(true));
        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        assert!(
            engine.gesture.needs_frame(),
            "the down arms a deadline, which is what keeps frames coming"
        );

        engine.clock.pin(0.6);
        let now = engine.clock.now_seconds();
        engine.service_gesture_clock(now);
        wait_for_log(&mut engine, "longpress:10");
        assert!(
            !engine.gesture.needs_frame(),
            "a resolved deadline stops asking for frames"
        );
    }

    /// A 200x200 scroller over two 200px rows, each logging its own tap into
    /// its own attribute — so a hit's row is observable from out here.
    const TWO_ROW_SCROLLER_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          const first = __CreateView(0);
          const second = __CreateView(0);
          __AppendElement(page, view);
          __AppendElement(view, first);
          __AppendElement(view, second);
          globalThis.held = [page, view, first, second];
          __SetInlineStyles(view,
            'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
          for (const row of [first, second]) {
            __SetInlineStyles(row,
              'flex-shrink:0;width:200px;height:200px;background-color:#808080');
          }
          __AddEventListener(first, 'tap', () => __SetAttribute(first, 'tapped', 'yes'), {});
          __AddEventListener(second, 'tap', () => __SetAttribute(second, 'tapped', 'yes'), {});
          __FlushElementTree();
        };
        ";

    /// The composed-scroll law, from the engine's side: a user scroll
    /// inside the encode window lands in the presenting side's intents and
    /// nowhere else — no command crosses, the document's offsets stay put,
    /// nothing recommits — and hit testing follows the intent offsets, not
    /// the committed ones, so a tap lands on what the screen shows.
    #[test]
    fn a_windowed_scroll_recommits_nothing_and_hits_route_at_the_intent_offsets() {
        let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
        let frame = engine.published_frame().expect("boot published a frame");
        assert!(
            frame.composite_plan().is_some(),
            "a scroller frame layers: targets draw it from retained planes"
        );
        let boot_commit = frame.commit_id();
        drop(frame);

        // 30px is inside half the encode-window headroom (the 200px
        // scrollport), so no refill commit is due either.
        engine.dispatch_input(InputEvent::wheel(
            Point2D::new(100.0, 100.0),
            dom::Vector2D::new(0.0, 30.0),
        ));
        assert_eq!(
            scroll_offset_of(&mut engine, 3),
            dom::Vector2D::zero(),
            "a windowed scroll leaves the document untouched"
        );
        // The probe round-tripped the main thread, so its round's
        // commit-if-dirty has already run — and found nothing.
        assert_eq!(
            engine
                .published_frame()
                .expect("still published")
                .commit_id(),
            boot_commit,
            "a windowed scroll must not recommit"
        );
        let scroller = node_id(3);
        assert_eq!(
            engine.scroll_intents.offset_for(scroller),
            Some(dom::Vector2D::new(0.0, 30.0)),
            "the intent carries the offset composition draws at"
        );

        // Screen y=180 plus the 30px intent offset is content y=210: the
        // second row. Routed against the committed offsets it would be the
        // first.
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(100.0, 180.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(100.0, 180.0),
            1,
            PointerKind::Touch,
            PointerPhase::Up,
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let second = attribute_of(&mut engine, 5, "tapped");
            if second.as_deref() == Some("yes") {
                break;
            }
            assert!(
                attribute_of(&mut engine, 4, "tapped").is_none(),
                "the tap landed on the unscrolled row: hits ignored the intent offsets"
            );
            assert!(
                Instant::now() < deadline,
                "the tap never delivered, second row saw {second:?}"
            );
            std::thread::yield_now();
        }
        assert!(
            attribute_of(&mut engine, 4, "tapped").is_none(),
            "only the row under the scrolled point may see the tap"
        );
    }

    /// A scroll past half the encode-window headroom asks the main thread
    /// for a refill: the next commit re-centers the windows and publishes
    /// the scrolled offsets, all without any script involvement.
    #[test]
    fn a_scroll_past_half_the_encode_window_requests_a_refill_commit() {
        let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
        let boot_commit = engine
            .published_frame()
            .expect("boot published a frame")
            .commit_id();

        // max_offset is 200 (400px of rows in a 200px scrollport), so the
        // window tops out at 200 and 150 is past half its headroom.
        engine.dispatch_input(InputEvent::wheel(
            Point2D::new(100.0, 100.0),
            dom::Vector2D::new(0.0, 150.0),
        ));

        let deadline = Instant::now() + Duration::from_secs(5);
        let frame = loop {
            let frame = engine.published_frame().expect("still published");
            if frame.commit_id() > boot_commit {
                break frame;
            }
            assert!(
                Instant::now() < deadline,
                "the refill commit never published"
            );
            std::thread::yield_now();
        };
        let scroller = node_id(3);
        let slot = frame.slot_of(scroller).expect("the scroller has a slot");
        let published = frame.scroll_slots()[slot as usize].offset;
        assert!(
            (published.y - 150.0).abs() < 0.5,
            "the refill commit publishes the scrolled offset, got {published:?}"
        );
    }

    /// Boots a card whose one view runs `animation_css`, waiting for the
    /// boot flush like [`booted`] does.
    fn booted_animated(animation_css: &str) -> OffscreenLynxView {
        let mut document = crate::tree::new_document(
            crate::tree::Viewport::new(393.0, 727.0),
            crate::tree::PageConfig::default(),
        );
        crate::style::add_style_sheet_text(&mut document, animation_css);
        let mut engine = super::LynxView::start(
            document,
            crate::tree::Viewport::new(393.0, 727.0),
            super::frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
            Arc::new(|| {}),
            super::EntryModule {
                source: r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __FlushElementTree();
                };
                "
                .to_owned(),
                url: "app:///animated.js".to_owned(),
            },
        )
        .expect("the test view starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !engine
            .pump()
            .into_iter()
            .any(|event| matches!(event, crate::EngineEvent::ScriptFinished))
        {
            assert!(Instant::now() < deadline, "the entry module did not finish");
            std::thread::yield_now();
        }
        engine
    }

    /// Sends one `BeginFrame` and waits for its round's commit to publish.
    fn synchronized_tick(engine: &mut OffscreenLynxView, now: f64) {
        let seq = engine.begin_frame(now, true).expect("a tick crosses");
        assert!(
            engine.hub.wait_begin_frame(seq, Duration::from_secs(5)),
            "the main thread services the tick"
        );
    }

    /// An exported curve animates on the presenting side: after the tick
    /// that promotes it to running, the committed frame carries the curve,
    /// wants no per-frame main-thread ticks, and `begin_frame` sends
    /// nothing.
    #[test]
    fn an_exported_curve_stops_asking_for_main_thread_ticks() {
        let mut engine = booted_animated(
            "view { width: 100px; height: 100px; background-color: red;
                    animation: fade 1s linear infinite; }
             @keyframes fade { from { opacity: 1; } to { opacity: 0; } }",
        );
        let boot = engine.published_frame().expect("the boot flush published");
        assert!(
            boot.needs_main_ticks(),
            "a pending animation still needs the promoting tick"
        );

        synchronized_tick(&mut engine, 0.1);
        let frame = engine.published_frame().expect("the promotion committed");
        assert!(frame.animations_active());
        assert!(frame.has_live_curves(), "the fade exported");
        assert!(
            !frame.needs_main_ticks(),
            "an exported curve frees the main thread"
        );
        assert!(
            engine.begin_frame(0.5, false).is_none(),
            "no BeginFrame crosses while the curve covers the animation"
        );
    }

    /// A finite curve's expiry is the one moment the main thread must hear
    /// about: the boundary tick runs the finish restyle and the next frame
    /// reports the timeline idle.
    #[test]
    fn a_finished_curve_hands_the_animation_back_to_the_main_thread() {
        let mut engine = booted_animated(
            "view { width: 100px; height: 100px; background-color: red;
                    animation: fade 0.2s linear; }
             @keyframes fade { from { opacity: 1; } to { opacity: 0; } }",
        );
        synchronized_tick(&mut engine, 0.05);
        let frame = engine.published_frame().expect("the promotion committed");
        assert!(frame.has_live_curves());
        assert!(
            engine.begin_frame(0.1, false).is_none(),
            "inside the curve's domain nothing crosses"
        );

        let seq = engine
            .begin_frame(0.3, false)
            .expect("the passed boundary sends the finish tick");
        assert!(engine.hub.wait_begin_frame(seq, Duration::from_secs(5)));
        let finished = engine.published_frame().expect("the finish committed");
        assert!(
            !finished.animations_active(),
            "the finish restyle retires the timeline"
        );
        assert!(!finished.has_live_curves());
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

        let mut first = booted(source);
        let mut second = booted(source);

        for engine in [&mut first, &mut second] {
            let children = engine
                .probe_document(|tree| tree.document_element().child_ids().len())
                .expect("each live view retains its own document");
            assert_eq!(children, 1);
        }
    }
}
