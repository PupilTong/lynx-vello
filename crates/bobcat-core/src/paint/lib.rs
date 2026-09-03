//! Painting ownership: input routing, scrolling, composition, and every
//! draw target the view has.
//!
//! Everything here runs on the thread that constructed the view — the
//! embedder's own — inside the calls the embedder makes. Its one link is
//! [`PainterLink`], to the Lynx main thread, and nothing it owns — a
//! surface, a scene buffer, a gesture arena — is ever touched from anywhere
//! else. [`Painter`] is `!Send` by construction, which is what makes the
//! constructing thread the painting thread for the view's whole life.

mod gesture;
mod graphics;
pub(crate) mod images;
mod sources;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod animation_tests;
#[cfg(test)]
mod event_loop_tests;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant as ClockInstant;

use dom::input::{InputEvent, InputKind};
use dom::render::gpu::Headless;
use dom::scroll::ScrollAxes;
use dom::vello::Scene;
use dom::vello::peniko::Color;
use dom::{CommittedFrame, HitTarget, NodeId, Vector2D};
use rustc_hash::{FxHashMap, FxHashSet};
#[cfg(target_arch = "wasm32")]
use web_time::Instant as ClockInstant;

use self::gesture::{EmitEvent, GestureRouter, InputDecision, InputDecisions, RouterHost};
pub use self::graphics::WindowTarget;
use self::graphics::{FrameAcquisition, WindowGraphics};
#[cfg(test)]
use crate::main::tree::LynxDocument;
use crate::main::tree::Viewport;
#[cfg(test)]
use crate::main::{EntryModule, MainLink, MainThreadHome, spawn_test_main_thread};
#[cfg(not(target_arch = "wasm32"))]
use crate::view::Screenshot;
use crate::view::{
    ComposeKey, DrawTarget, EngineError, EngineEvent, FrameHub, FrameSize, LoadedSource,
    LynxViewError, ToMain, ToPainter, frame_slot,
};
#[cfg(test)]
use crate::view::{NoWakeup, main_link};

const BEGIN_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// How the wait on one host fetch ended.
enum FetchOutcome {
    Loaded(LoadedSource),
    /// Startup is decided — either the fetch itself failed, which *is* the
    /// failure, or the main thread spoke first and this fetch is abandoned.
    Ended(Result<(), LynxViewError>),
}

/// Which of the two things a fetch waits on happened first.
enum FetchStep {
    Loaded(Result<LoadedSource, LynxViewError>),
    /// `bobcat-main` spoke, or stopped being able to.
    Received(Result<ToPainter, flume::RecvError>),
}

/// The main thread stopped before it could say how startup went.
fn main_thread_gone() -> LynxViewError {
    EngineError::Thread {
        name: "script",
        message: "the Lynx main thread stopped before startup completed".to_owned(),
    }
    .into()
}

/// The painter's monotonic animation timeline. Its epoch is view
/// construction, and one reading is shared by every operation in a frame.
#[derive(Debug)]
pub(crate) struct FrameClock {
    epoch: ClockInstant,
    #[cfg(test)]
    pinned: Option<f64>,
}

impl FrameClock {
    pub(crate) fn new() -> Self {
        Self {
            epoch: ClockInstant::now(),
            #[cfg(test)]
            pinned: None,
        }
    }

    pub(crate) fn now_seconds(&self) -> f64 {
        #[cfg(test)]
        if let Some(seconds) = self.pinned {
            return seconds;
        }
        self.epoch.elapsed().as_secs_f64()
    }

    #[cfg(test)]
    pub(crate) fn pin(&mut self, seconds: f64) {
        self.pinned = Some(seconds.max(0.0));
    }
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod clock_tests {
    use super::FrameClock;

    #[test]
    fn the_clock_starts_near_zero_and_never_goes_back() {
        let clock = FrameClock::new();
        let first = clock.now_seconds();
        let second = clock.now_seconds();
        assert!((0.0..1.0).contains(&first), "the epoch is clock creation");
        assert!(second >= first, "and time only moves forward");
    }

    #[test]
    fn a_pinned_clock_holds_the_instant_a_test_named() {
        let mut clock = FrameClock::new();
        clock.pin(1.5);
        assert!((clock.now_seconds() - 1.5).abs() < 1e-9);
        clock.pin(1.75);
        assert!((clock.now_seconds() - 1.75).abs() < 1e-9);
    }

    #[test]
    fn a_pinned_clock_never_reports_negative_time() {
        let mut clock = FrameClock::new();
        clock.pin(-5.0);
        assert!((clock.now_seconds() - 0.0).abs() < f64::EPSILON);
    }
}

/// The painting end of the view's one link — to the Lynx main thread —
/// including the replicas it uses while routing and drawing without touching
/// that thread.
pub(crate) struct PainterLink {
    commands: flume::Sender<ToMain>,
    notifications: flume::Receiver<ToPainter>,
    frames: Arc<FrameHub>,
    frame: Option<Arc<CommittedFrame>>,
    events: Vec<EngineEvent>,
    listener_names: FxHashSet<Arc<str>>,
    begin_frames_sent: u64,
    begin_frames_serviced: u64,
    redraw_pending: Cell<bool>,
    /// Sources the document met and wants named. Buffered here because
    /// asking for them needs the host's resource system, which the painter
    /// owns rather than the link.
    image_requests: Vec<Arc<str>>,
    /// Whether a drain has seen a frame announcement it has not adopted yet.
    /// A field rather than a local because a startup drain runs in pieces.
    pending_announce: bool,
}

impl PainterLink {
    pub(crate) fn new(
        commands: flume::Sender<ToMain>,
        notifications: flume::Receiver<ToPainter>,
        frames: Arc<FrameHub>,
    ) -> Self {
        Self {
            commands,
            notifications,
            frames,
            frame: None,
            events: Vec::new(),
            listener_names: FxHashSet::default(),
            begin_frames_sent: 0,
            begin_frames_serviced: 0,
            redraw_pending: Cell::new(false),
            image_requests: Vec::new(),
            pending_announce: false,
        }
    }

    /// Adopts whatever the drains since the last settle announced.
    fn settle(&mut self) {
        let announced = std::mem::take(&mut self.pending_announce);
        self.adopt_frame(announced);
    }

    /// Sends one command. A closed channel is a main thread that has exited;
    /// the painter goes on showing what it last published.
    pub(crate) fn send(&self, command: ToMain) {
        let _ = self.commands.send(command);
    }

    /// Applies everything that has arrived. However many frames were
    /// announced, the mailbox is read once.
    pub(crate) fn sync(&mut self) {
        while let Ok(notification) = self.notifications.try_recv() {
            self.apply(notification);
        }
        self.settle();
    }

    fn apply(&mut self, notification: ToPainter) {
        match notification {
            ToPainter::FrameChanged => self.pending_announce = true,
            ToPainter::Engine(event) => self.events.push(event),
            ToPainter::ListenerAvailable(name) => {
                self.listener_names.insert(name);
            }
            ToPainter::ListenerUnavailable(name) => {
                self.listener_names.remove(&name);
            }
            ToPainter::BeginFrameServiced(seq) => {
                self.begin_frames_serviced = self.begin_frames_serviced.max(seq);
            }
            ToPainter::RequestImages(sources) => self.image_requests.extend(sources),
            ToPainter::Started(_) => {
                unreachable!("startup messages are served before the view exists")
            }
        }
    }

    fn take_image_requests(&mut self) -> Vec<Arc<str>> {
        std::mem::take(&mut self.image_requests)
    }

    fn adopt_frame(&mut self, announced: bool) {
        if announced {
            self.frame.clone_from(&frame_slot(&self.frames));
            self.redraw_pending.set(true);
        }
    }

    pub(crate) fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.frame.as_ref()
    }

    pub(crate) fn has_listener(&self, name: &str) -> bool {
        self.listener_names.contains(name)
    }

    pub(crate) fn take_events(&mut self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.events)
    }

    /// Marks a redraw the painter owes itself. It wakes nobody: every caller
    /// is on the host's own thread, inside the host's own call, so the turn
    /// that host is already in is the turn that answers it.
    pub(crate) fn mark_redraw(&self) {
        self.redraw_pending.set(true);
    }

    pub(crate) fn take_redraw(&self) -> bool {
        self.redraw_pending.replace(false)
    }

    pub(crate) fn redraw_owed(&self) -> bool {
        self.redraw_pending.get()
    }

    pub(crate) fn begin_frame(&mut self, now: f64) -> Option<u64> {
        self.begin_frames_sent += 1;
        let seq = self.begin_frames_sent;
        self.commands
            .send(ToMain::BeginFrame { now, seq })
            .ok()
            .map(|()| seq)
    }

    /// Waits for a particular main-thread animation round while applying all
    /// notifications that precede its acknowledgement.
    ///
    /// The one blocking wait a host's own thread makes on `bobcat-main`, and
    /// `tick` — offscreen only — is the one call that reaches it. Its
    /// `recv_timeout` reads the standard library's clock, which no wasm32
    /// target implements, so an offscreen view belongs to a native host.
    pub(crate) fn wait_begin_frame(&mut self, seq: u64, timeout: Duration) -> bool {
        let deadline = ClockInstant::now() + timeout;
        while self.begin_frames_serviced < seq {
            let Some(remaining) = deadline.checked_duration_since(ClockInstant::now()) else {
                break;
            };
            let Ok(notification) = self.notifications.recv_timeout(remaining) else {
                break;
            };
            self.apply(notification);
        }
        self.settle();
        self.begin_frames_serviced >= seq
    }

    #[cfg(test)]
    pub(crate) fn drain(&mut self) -> Vec<ToPainter> {
        self.notifications.drain().collect()
    }

    /// Waits for one host fetch **while still watching the inbox**.
    ///
    /// A fetch is the one await in construction that is not on the link, and
    /// `bobcat-main` mounts each pushed source while the next fetch is
    /// already in flight — so a failure it decides there, or a trap's last
    /// words, can land while the host is still holding the answer. Polling
    /// the inbox *beside* the fetch — rather than in place of it — is what
    /// keeps an outcome that has already been decided observable.
    async fn await_fetch(
        &mut self,
        load: impl Future<Output = Result<LoadedSource, LynxViewError>>,
    ) -> FetchOutcome {
        let mut load = std::pin::pin!(load);
        // A second handle on the same queue, so the wait does not borrow the
        // link that handling a message needs. Dropping a pending `recv_async`
        // deregisters its waker and takes no message with it, so abandoning
        // one to serve the fetch loses nothing.
        let inbox = self.notifications.clone();
        loop {
            let mut next = std::pin::pin!(inbox.recv_async());
            let step = std::future::poll_fn(|context| {
                if let Poll::Ready(loaded) = load.as_mut().poll(context) {
                    return Poll::Ready(FetchStep::Loaded(loaded));
                }
                // Registers the waker on the inbox too, so anything the main
                // thread says resumes this even though the fetch has not.
                next.as_mut().poll(context).map(FetchStep::Received)
            })
            .await;
            match step {
                FetchStep::Loaded(Ok(source)) => return FetchOutcome::Loaded(source),
                FetchStep::Loaded(Err(error)) => return FetchOutcome::Ended(Err(error)),
                FetchStep::Received(Err(flume::RecvError::Disconnected)) => {
                    self.settle();
                    return FetchOutcome::Ended(Err(main_thread_gone()));
                }
                FetchStep::Received(Ok(notification)) => {
                    if let Some(result) = self.take_startup(notification) {
                        return FetchOutcome::Ended(result);
                    }
                }
            }
        }
    }

    /// Waits for one thing `bobcat-main` has to say, and applies it.
    ///
    /// The tail of construction: every source has been pushed, so the only
    /// thing that can move startup on is a message.
    async fn await_startup(&mut self) -> Option<Result<(), LynxViewError>> {
        let inbox = self.notifications.clone();
        match inbox.recv_async().await {
            Ok(notification) => self.take_startup(notification),
            Err(flume::RecvError::Disconnected) => {
                self.settle();
                Some(Err(main_thread_gone()))
            }
        }
    }

    /// What one message means during construction. `Some` once startup has an
    /// outcome.
    ///
    /// The single place that decides; both readers feed it, so awaiting one
    /// message and draining a queue of them cannot disagree.
    fn take_startup(&mut self, notification: ToPainter) -> Option<Result<(), LynxViewError>> {
        match notification {
            ToPainter::Started(result) => {
                self.settle();
                Some(result)
            }
            // A script error during startup *is* the startup failure. On
            // wasm32 under `panic = "abort"` it is the only thing a trapping
            // main thread can say before it stops running destructors, so
            // treating it as terminal here is what keeps construction from
            // waiting forever.
            ToPainter::Engine(EngineEvent::ScriptRunError(error)) => {
                self.settle();
                Some(Err(error.into()))
            }
            // Frames, lifecycle events, listener edges, boot's image
            // requests: the steady-state path, so every fact boot published
            // lands where the host's first turn finds it.
            other => {
                self.apply(other);
                None
            }
        }
    }
}

impl fmt::Debug for PainterLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PainterLink")
            .field("listener_names", &self.listener_names.len())
            .field("begin_frames_sent", &self.begin_frames_sent)
            .finish_non_exhaustive()
    }
}

/// Where a view's pixels go: a window's presentation stack, or a texture the
/// view owns and nothing displays. One of them exists before the view does,
/// and it is the one the view has for its whole life.
pub(crate) enum Output {
    /// A painter with nowhere to draw. Test-only, so a unit test that
    /// exercises routing alone pays for no GPU device; production has
    /// exactly the two targets an embedder can name.
    #[cfg(test)]
    None,
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            dead_code,
            reason = "a browser view is refused this target at construction"
        )
    )]
    Offscreen(Box<Headless>),
    Window(Box<WindowGraphics>),
}

impl Output {
    /// Builds the target an embedder named, on the thread that will draw into
    /// it — the only thread macOS lets a surface be created from.
    pub(crate) async fn build(
        target: DrawTarget,
        frame_size: FrameSize,
    ) -> Result<Self, EngineError> {
        match target {
            DrawTarget::Window(target) => Ok(Self::Window(Box::new(
                WindowGraphics::new(target, frame_size).await?,
            ))),
            DrawTarget::Offscreen => Self::offscreen(),
        }
    }

    /// A windowless GPU target.
    ///
    /// `Headless::new` blocks on a device request, and a browser Worker is
    /// the thread whose event loop would have answered it — so rather than
    /// hang, a Wasm view is told no.
    fn offscreen() -> Result<Self, EngineError> {
        #[cfg(target_arch = "wasm32")]
        return Err(EngineError::Gpu(
            "an offscreen target blocks the thread that builds it on a device \
             request; a browser Worker is the thread that would answer it"
                .to_owned(),
        ));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let gpu = Headless::new().map_err(|error| EngineError::Gpu(error.to_string()))?;
            Ok(Self::Offscreen(Box::new(gpu)))
        }
    }
}

/// The painting half of a running view, on the thread that owns it.
///
/// Kept on that thread by construction — the `Rc` marker makes the whole
/// struct `!Send`, and [`crate::LynxView`] owns one by value, so the thread
/// that built the view is the only one that can ever draw for it.
/// The painter every in-crate test builds: no test here is about the host's
/// resource system, so they all share the one that answers nothing.
#[cfg(test)]
pub(crate) type TestPainter = Painter<crate::resource::NeverAnswers>;

pub(crate) struct Painter<F> {
    // Keep first: dropping the link closes the sole command sender, which
    // wakes the Lynx main thread before any state it may still refer to is
    // released.
    pub(super) link: PainterLink,
    #[cfg(test)]
    main: Option<MainThreadHome>,
    #[cfg(test)]
    pub(super) detached: bool,
    viewport: Viewport,
    frame_size: FrameSize,
    output: Output,
    /// A window nobody can see draws nothing; the frame it owes stays owed.
    occluded: bool,
    /// A draw target that failed once cannot be reached again: it is reported
    /// once, and nothing tries to paint it until another target arrives.
    render_failed: bool,
    pub(super) gesture: GestureRouter,
    pub(super) clock: FrameClock,
    pub(super) scroll_intents: ScrollIntents,
    composed: Option<ComposeKey>,
    composed_scene: Scene,
    refill_requested_for: Option<u64>,
    /// The whole image resource system. Owned here and nowhere else.
    images: images::PainterImages<F>,
    thread_bound: PhantomData<Rc<()>>,
}

impl<F> std::fmt::Debug for Painter<F> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Painter")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl<F> Drop for Painter<F> {
    fn drop(&mut self) {
        if let Some(main) = self.main.as_mut() {
            self.link.send(ToMain::Shutdown);
            main.shutdown();
        }
    }
}

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

struct FrameRouterHost<'a> {
    frame: Option<&'a CommittedFrame>,
    link: &'a PainterLink,
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
        self.link.has_listener(name)
    }
}

#[derive(Debug, Default)]
pub(super) struct ScrollIntents {
    pub(super) offsets: FxHashMap<NodeId, Vector2D<f32>>,
    rebased_commit: Option<u64>,
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
            *offset != slot.offset
        });
    }

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

    pub(super) fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>> {
        self.offsets.get(&node).copied()
    }

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

    fn writeback(&self) -> Vec<(NodeId, Vector2D<f32>)> {
        self.offsets
            .iter()
            .map(|(node, offset)| (*node, *offset))
            .collect()
    }
}

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

fn scene_for<'frame>(
    intents: &ScrollIntents,
    buffer: &'frame mut Scene,
    frame: &'frame CommittedFrame,
    images: &[Option<dom::vello::peniko::ImageData>],
    animation_now: Option<f64>,
) -> &'frame Scene {
    if intents.offsets.is_empty()
        && animation_now.is_none()
        && let Some(scene) = frame.scene()
    {
        return scene;
    }
    buffer.reset();
    frame.compose_into(
        buffer,
        images,
        &|slot| intents.offset_for(slot.node),
        animation_now,
    );
    buffer
}

fn composite_scene<'frame>(
    intents: &ScrollIntents,
    buffer: &'frame mut Scene,
    frame: &CommittedFrame,
    plane_images: &[dom::vello::peniko::ImageData],
    images: &[Option<dom::vello::peniko::ImageData>],
    animation_now: Option<f64>,
) -> &'frame Scene {
    buffer.reset();
    frame.composite_into(
        buffer,
        plane_images,
        images,
        &|slot| intents.offset_for(slot.node),
        animation_now,
    );
    buffer
}

#[expect(
    clippy::too_many_arguments,
    reason = "one compose call's full inputs, none of which the painter owns together"
)]
fn paint_window(
    graphics: &mut WindowGraphics,
    intents: &ScrollIntents,
    buffer: &mut Scene,
    frame: &CommittedFrame,
    images: &[Option<dom::vello::peniko::ImageData>],
    size: FrameSize,
    key: ComposeKey,
    animation_now: Option<f64>,
) -> Result<(), EngineError> {
    if animation_now.is_none() && !graphics.needs_paint(key, size) {
        return Ok(());
    }
    let scene = if frame.composite_plan().is_some() {
        graphics.prepare_planes(frame, images)?;
        composite_scene(
            intents,
            buffer,
            frame,
            graphics.plane_images(),
            images,
            animation_now,
        )
    } else {
        scene_for(intents, buffer, frame, images, animation_now)
    };
    graphics.render_to_target(scene, size, key)
}

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

impl<F> Painter<F> {
    /// Tells the Lynx main thread to stop. Its command loop returns on this
    /// message, which is why it is sent explicitly rather than left to the
    /// FIFO closing when the painter is released.
    ///
    /// Teardown knows nothing about the store, so it stays reachable for any
    /// `F` — which is what lets `Drop` run without the trait bound.
    pub(super) fn shutdown(&self) {
        // Close the sink before the store drops: a loader still in flight
        // must find it detached rather than queue into a dead view.
        self.images.detach();
        self.link.send(ToMain::Shutdown);
    }
}

/// The test constructors pin the fetcher: no in-crate test is about the
/// host's resource system, so they all build over the one that answers
/// nothing.
#[cfg(test)]
impl TestPainter {
    pub(super) fn start<R: crate::view::EventRequester>(
        document: LynxDocument,
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<R>,
        entry: EntryModule,
        output: Output,
    ) -> Result<Self, EngineError> {
        let (mut painter, main) = Self::with_link(viewport, frame_size, event_requester, output);
        painter.main = Some(spawn_test_main_thread(document, entry, main)?);
        painter.detached = false;
        Ok(painter)
    }

    pub(crate) fn probe_document<T: Send + 'static>(
        &mut self,
        probe: impl FnOnce(&mut LynxDocument) -> T + Send + 'static,
    ) -> Option<T> {
        if self.detached {
            return None;
        }
        let (sender, receiver) = flume::unbounded();
        self.link.send(ToMain::Probe(Box::new(move |document| {
            let _ = sender.send(probe(document));
        })));
        receiver.recv_timeout(Duration::from_secs(10)).ok()
    }

    #[cfg(test)]
    pub(crate) fn published_frame(&mut self) -> Option<Arc<CommittedFrame>> {
        self.sync();
        self.link.frame().cloned()
    }

    /// The painter and the other end of its link, with no Lynx main thread
    /// started over it yet — the seam `start` spawns through, and the one a
    /// test plays the main thread's half of.
    #[cfg(test)]
    pub(super) fn with_link<R: crate::view::EventRequester>(
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<R>,
        output: Output,
    ) -> (Self, MainLink<R>) {
        let (link, main) = main_link(event_requester);
        let painter = Self::with_output(viewport, frame_size, link, output, |_reports| {
            crate::resource::NeverAnswers
        });
        (painter, main)
    }
}

impl<F: crate::resource::ResourceFetcher> Painter<F> {
    pub(super) fn with_output<B>(
        viewport: Viewport,
        frame_size: FrameSize,
        link: PainterLink,
        output: Output,
        resources: B,
    ) -> Self
    where
        B: FnOnce(dom::ImageReports) -> F,
    {
        Self {
            link,
            #[cfg(test)]
            main: None,
            #[cfg(test)]
            detached: true,
            viewport,
            frame_size,
            output,
            occluded: false,
            render_failed: false,
            gesture: GestureRouter::default(),
            clock: FrameClock::new(),
            scroll_intents: ScrollIntents::default(),
            composed: None,
            composed_scene: Scene::new(),
            refill_requested_for: None,
            images: images::PainterImages::new(resources),
            thread_bound: PhantomData,
        }
    }

    /// Pushes every source to `bobcat-main`, then waits until it answers
    /// startup.
    ///
    /// This is the painter's only wait on the Lynx main thread during
    /// construction, and it is a drain of the same one inbox the steady state
    /// drains. The order of the pushes is the protocol: sheets in cascade
    /// order, the entry last, so the receiving side mounts in arrival order
    /// and boots on the entry's arrival.
    ///
    /// It cannot deadlock against `bobcat-main`: that thread waits on
    /// nothing but this loop's own sends, and this loop's fetches wait on
    /// the host, never on that thread.
    pub(super) async fn serve_startup(
        &mut self,
        style_sheets: Vec<String>,
        entry: String,
    ) -> Result<(), LynxViewError> {
        // Split borrows: the fetch borrows the store across an await while
        // the same loop keeps draining the link.
        let Self { link, images, .. } = self;
        let fetcher = images.store();
        let mut requests = sources::mint_namespace();
        for specifier in &style_sheets {
            let load = sources::load_style_sheet(fetcher, &mut requests, specifier);
            match link.await_fetch(load).await {
                FetchOutcome::Loaded(source) => link.send(ToMain::SourceLoaded { source }),
                // A fetch failure is the startup failure and the painter is
                // already holding it: sending it across to be told back would
                // be a round trip to learn what we just decided.
                FetchOutcome::Ended(result) => return result,
            }
        }
        match link
            .await_fetch(sources::load_entry(fetcher, &mut requests, &entry))
            .await
        {
            FetchOutcome::Loaded(source) => link.send(ToMain::SourceLoaded { source }),
            FetchOutcome::Ended(result) => return result,
        }
        loop {
            // Everything is pushed, so the only thing that can move startup
            // on is a message. A main thread that has gone is a receive
            // error here, not a separate flag to carry.
            if let Some(result) = link.await_startup().await {
                return result;
            }
        }
    }

    /// Warms sources the walk has not met yet.
    pub(super) fn prefetch_images(&mut self, sources: Vec<Arc<str>>) {
        self.images.request(sources);
    }

    /// One painter turn's intake: everything the document said, then the
    /// image work that came with it.
    ///
    /// The two are one call because they are one fact. A turn that drained
    /// the link without servicing its image requests would leave the store
    /// unasked, and the frame that needed those images would never arrive.
    fn sync(&mut self) {
        self.link.sync();
        self.service_images();
    }

    /// Services the image protocol: gives the host its moment in the turn,
    /// asks it for every source the document met, and forwards any completed
    /// loads back to the document.
    fn service_images(&mut self) {
        self.images.service();
        self.images.request(self.link.take_image_requests());
        let events = self.images.take_reports();
        if !events.is_empty() {
            self.link.send(ToMain::ImageEvents(events));
        }
    }

    /// Whether the engine owes the timeline another frame, as of the last
    /// pass that drained the link — a `serve`, a `draw`, or an input. That is
    /// when a host asks: after answering the wakeup that carried the frame.
    #[must_use]
    pub(super) fn is_animating(&self) -> bool {
        self.link
            .frame()
            .is_some_and(|frame| frame.animations_active())
            || self.gesture.needs_frame()
    }

    pub(super) fn dispatch_input(&mut self, event: InputEvent) {
        self.sync();
        let at = self.clock.now_seconds();
        let published = self.link.frame().cloned();
        if let Some(frame) = &published {
            self.scroll_intents.rebase(frame);
        }
        let generation = self.scroll_intents.generation;
        let frame = published.as_deref();
        let animation_now = frame.and_then(|frame| frame.has_live_curves().then_some(at));
        let target = route_published(frame, &self.scroll_intents, &event, animation_now);
        let mut decisions = InputDecisions::new();
        {
            let host = FrameRouterHost {
                frame,
                link: &self.link,
            };
            self.gesture
                .on_input(&event, target, at, &host, &mut decisions);
        }
        self.execute_decisions(&mut decisions, published.as_deref());
        if let Some(frame) = &published {
            self.maybe_request_refill(frame);
        }
        if self.gesture.needs_frame() || self.scroll_intents.generation != generation {
            self.refresh();
        }
    }

    fn maybe_request_refill(&mut self, frame: &CommittedFrame) {
        if self.refill_requested_for == Some(frame.commit_id())
            || !self.scroll_intents.refill_due(frame)
        {
            return;
        }
        self.refill_requested_for = Some(frame.commit_id());
        self.link.send(ToMain::Refill {
            offsets: self.scroll_intents.writeback(),
        });
    }

    pub(super) fn execute_decisions(
        &mut self,
        decisions: &mut InputDecisions,
        published: Option<&CommittedFrame>,
    ) {
        let Self {
            link,
            gesture,
            scroll_intents,
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
                    if !link.has_listener(event.name) {
                        continue;
                    }
                    link.send(ToMain::DispatchEvent {
                        target: event.target,
                        name: event.name,
                        detail: emit_detail(&event),
                    });
                }
            }
        }
    }

    pub(super) fn service_gesture_clock(&mut self, now: f64) {
        self.sync();
        let published = self.link.frame().cloned();
        let mut decisions = InputDecisions::new();
        {
            let host = FrameRouterHost {
                frame: published.as_deref(),
                link: &self.link,
            };
            self.gesture.on_tick(now, &host, &mut decisions);
        }
        self.execute_decisions(&mut decisions, published.as_deref());
    }

    /// Applies new device metrics, if they moved at all.
    ///
    /// The size is validated first, so a target the painter could not render
    /// is refused before anything else has seen it.
    pub(super) fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), EngineError> {
        let next_size = FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let moved = self.viewport.width.to_bits() != width.to_bits()
            || self.viewport.height.to_bits() != height.to_bits()
            || self.viewport.device_pixel_ratio.to_bits() != device_pixel_ratio.to_bits();
        if !moved {
            return Ok(());
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.link.send(ToMain::Resize {
            width,
            height,
            device_pixel_ratio,
        });
        self.refresh();
        Ok(())
    }

    pub(super) const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    pub(super) fn refresh(&self) {
        self.link.mark_redraw();
    }

    /// A window nobody can see draws nothing, and un-occluding asks again
    /// for the frame that was held back.
    pub(super) fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        if !occluded {
            self.refresh();
        }
    }

    #[must_use]
    pub(super) fn pump(&mut self) -> Vec<EngineEvent> {
        self.sync();
        self.link.take_events()
    }

    pub(super) fn begin_frame(&mut self, now: f64, always: bool) -> Option<u64> {
        #[cfg(test)]
        if self.detached {
            return None;
        }
        let main_ticks_due = self
            .link
            .frame()
            .is_some_and(|frame| frame.needs_main_ticks() || frame.animation_boundary_passed(now));
        if !main_ticks_due && !always {
            return None;
        }
        self.link.begin_frame(now)
    }

    pub(super) fn draw(&mut self) -> Result<(), EngineError> {
        if !matches!(self.output, Output::Window(_)) || self.occluded {
            return Ok(());
        }
        self.sync();
        if !self.link.take_redraw() && !self.is_animating() {
            return Ok(());
        }
        let size = self.frame_size;
        // Resolving reads pixels, and a store is allowed to block restoring
        // one it evicted. That must happen before a swap-chain image is
        // acquired: blocking while holding one stalls the chain under vsync.
        let latest = self.link.frame().cloned();
        if let Some(frame) = &latest {
            self.images.resolve(frame);
        }
        let acquired = {
            let Output::Window(graphics) = &mut self.output else {
                unreachable!("the window output was just checked");
            };
            match graphics.acquire(size)? {
                FrameAcquisition::Ready(acquired) => acquired,
                // No image this frame, and no vsync was waited on to find
                // that out. The frame stays owed and the host takes it at its
                // next display frame, like any other — which is what keeps an
                // empty swap chain from spinning: nothing here asks to come
                // straight back.
                FrameAcquisition::Retry => {
                    self.link.mark_redraw();
                    return Ok(());
                }
            }
        };
        let now = self.clock.now_seconds();
        self.service_gesture_clock(now);
        let _ = self.begin_frame(now, false);
        if let Some(frame) = &latest {
            self.scroll_intents.rebase(frame);
            self.maybe_request_refill(frame);
        }
        let key = latest
            .as_ref()
            .map(|frame| (frame.commit_id(), self.scroll_intents.generation));
        let animation_now = latest
            .as_ref()
            .and_then(|frame| frame.has_live_curves().then_some(now));
        let images = self.images.resolved();
        let Output::Window(graphics) = &mut self.output else {
            unreachable!("the window output was just checked");
        };
        if let (Some(frame), Some(key)) = (&latest, key) {
            paint_window(
                graphics,
                &self.scroll_intents,
                &mut self.composed_scene,
                frame,
                images,
                size,
                key,
                animation_now,
            )?;
        }
        if graphics.rendered_at(size) {
            graphics.present(acquired);
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn capture(&mut self) -> Result<Screenshot, EngineError> {
        let size = self.frame_size;
        let now = self.clock.now_seconds();
        self.sync();
        let latest = self.link.frame().cloned();
        if let Some(frame) = &latest {
            self.images.resolve(frame);
        }
        let key = latest
            .as_ref()
            .map(|frame| (frame.commit_id(), self.scroll_intents.generation));
        let animation_now = latest
            .as_ref()
            .and_then(|frame| frame.has_live_curves().then_some(now));
        let images = self.images.resolved();
        match &mut self.output {
            #[cfg(test)]
            Output::None => Err(EngineError::NotOffscreen),
            Output::Offscreen(gpu) => {
                if let (Some(frame), Some(key)) = (&latest, key)
                    && (self.composed != Some(key) || animation_now.is_some())
                {
                    let scene = if frame.composite_plan().is_some() {
                        gpu.prepare_planes(frame, images)
                            .map_err(|error| EngineError::Gpu(error.to_string()))?;
                        composite_scene(
                            &self.scroll_intents,
                            &mut self.composed_scene,
                            frame,
                            gpu.plane_images(),
                            images,
                            animation_now,
                        )
                    } else {
                        scene_for(
                            &self.scroll_intents,
                            &mut self.composed_scene,
                            frame,
                            images,
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
                if let (Some(frame), Some(key)) = (&latest, key) {
                    paint_window(
                        graphics,
                        &self.scroll_intents,
                        &mut self.composed_scene,
                        frame,
                        images,
                        size,
                        key,
                        animation_now,
                    )?;
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
    /// Runs one turn: produce the frame it owes, and hand back everything
    /// the realm had to say.
    ///
    /// In that order deliberately. Drawing first means the pixels a fatal
    /// script error left behind reach the screen on the turn that reports
    /// it, with nobody left to ask for another frame.
    ///
    /// A draw that fails is reported once. There is no recovering a lost
    /// surface, and the turn would otherwise report the same failure for as
    /// long as the host takes to notice the first.
    #[must_use]
    pub(super) fn serve(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        if !self.render_failed
            && let Err(error) = self.draw()
        {
            self.render_failed = true;
            events.push(EngineEvent::RenderFailed(error));
        }
        events.append(&mut self.pump());
        events
    }

    /// Whether the view has a frame to put on its window.
    ///
    /// A running animation, a swap chain that had no image to give, and a
    /// frame something asked for that no turn has produced yet are one
    /// answer, because a host serves them all the same way: at its own next
    /// display frame. No delay is named here — the display's clock belongs to
    /// the embedder, and this is the whole of what the engine has to say
    /// about when to read it.
    ///
    /// Always false for a window nobody can see, a target that failed, and a
    /// view that presents to no window at all: an offscreen view's frames are
    /// the host's to ask for through `tick`.
    pub(super) fn owes_frame(&self) -> bool {
        if self.render_failed || self.occluded || !matches!(self.output, Output::Window(_)) {
            return false;
        }
        self.is_animating() || self.link.redraw_owed()
    }

    /// Advances an offscreen view by one frame.
    ///
    /// Offscreen only, and the check is load-bearing: this is the one call
    /// that blocks the embedder's own thread on `bobcat-main`, and a windowed
    /// view's frames come from `pump` instead.
    pub(super) fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        if !matches!(self.output, Output::Offscreen(_)) {
            return Err(EngineError::NotOffscreen);
        }
        let now = self.clock.now_seconds();
        self.service_gesture_clock(now);
        if let Some(seq) = self.begin_frame(now, true) {
            let _ = self.link.wait_begin_frame(seq, BEGIN_FRAME_TIMEOUT);
        }
        self.sync();
        let Some(frame) = self.link.frame().cloned() else {
            return Ok(false);
        };
        self.scroll_intents.rebase(&frame);
        self.maybe_request_refill(&frame);
        self.images.resolve(&frame);
        let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
        let animation_now = frame.has_live_curves().then_some(now);
        if self.composed == Some(key) && !force && animation_now.is_none() {
            return Ok(false);
        }
        let images = self.images.resolved();
        let Output::Offscreen(gpu) = &mut self.output else {
            unreachable!("the offscreen output was just checked");
        };
        let scene = if frame.composite_plan().is_some() {
            gpu.prepare_planes(&frame, images)
                .map_err(|error| EngineError::Gpu(error.to_string()))?;
            composite_scene(
                &self.scroll_intents,
                &mut self.composed_scene,
                &frame,
                gpu.plane_images(),
                images,
                animation_now,
            )
        } else {
            scene_for(
                &self.scroll_intents,
                &mut self.composed_scene,
                &frame,
                images,
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
