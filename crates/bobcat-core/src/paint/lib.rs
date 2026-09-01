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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod animation_tests;
#[cfg(test)]
mod event_loop_tests;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
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
    ComposeKey, DrawTarget, EngineError, EngineEvent, FrameHub, FrameSize, ToMain, ToPainter,
    frame_slot,
};
#[cfg(test)]
use crate::view::{EventRequester, NoWakeup, main_link};

const BEGIN_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

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
    commands: mpsc::Sender<ToMain>,
    notifications: mpsc::Receiver<ToPainter>,
    frames: Arc<FrameHub>,
    frame: Option<Arc<CommittedFrame>>,
    events: Vec<EngineEvent>,
    listener_names: FxHashSet<Arc<str>>,
    begin_frames_sent: u64,
    begin_frames_serviced: u64,
    redraw_pending: Cell<bool>,
}

impl PainterLink {
    pub(crate) fn new(
        commands: mpsc::Sender<ToMain>,
        notifications: mpsc::Receiver<ToPainter>,
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
        }
    }

    /// Sends one command. A closed channel is a main thread that has exited;
    /// the painter goes on showing what it last published.
    pub(crate) fn send(&self, command: ToMain) {
        let _ = self.commands.send(command);
    }

    /// Applies everything that has arrived. However many frames were
    /// announced, the mailbox is read once.
    pub(crate) fn sync(&mut self) {
        let mut announced = false;
        while let Ok(notification) = self.notifications.try_recv() {
            announced |= self.apply(notification);
        }
        self.adopt_frame(announced);
    }

    fn apply(&mut self, notification: ToPainter) -> bool {
        match notification {
            ToPainter::FrameChanged => return true,
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
        }
        false
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
        let mut announced = false;
        while self.begin_frames_serviced < seq {
            let Some(remaining) = deadline.checked_duration_since(ClockInstant::now()) else {
                break;
            };
            let Ok(notification) = self.notifications.recv_timeout(remaining) else {
                break;
            };
            announced |= self.apply(notification);
        }
        self.adopt_frame(announced);
        self.begin_frames_serviced >= seq
    }

    #[cfg(test)]
    pub(crate) fn drain(&mut self) -> Vec<ToPainter> {
        self.notifications.try_iter().collect()
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
pub(super) enum Output {
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
    async fn build(target: DrawTarget, frame_size: FrameSize) -> Result<Self, EngineError> {
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
pub(crate) struct Painter {
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
    thread_bound: PhantomData<Rc<()>>,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Painter")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl Drop for Painter {
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
    animation_now: Option<f64>,
) -> &'frame Scene {
    if intents.offsets.is_empty()
        && animation_now.is_none()
        && let Some(scene) = frame.scene()
    {
        return scene;
    }
    buffer.reset();
    frame.compose_into(buffer, &|slot| intents.offset_for(slot.node), animation_now);
    buffer
}

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

fn paint_window(
    graphics: &mut WindowGraphics,
    intents: &ScrollIntents,
    buffer: &mut Scene,
    frame: &CommittedFrame,
    size: FrameSize,
    key: ComposeKey,
    animation_now: Option<f64>,
) -> Result<(), EngineError> {
    if animation_now.is_none() && !graphics.needs_paint(key, size) {
        return Ok(());
    }
    let scene = if frame.composite_plan().is_some() {
        graphics.prepare_planes(frame)?;
        composite_scene(
            intents,
            buffer,
            frame,
            graphics.plane_images(),
            animation_now,
        )
    } else {
        scene_for(intents, buffer, frame, animation_now)
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

impl Painter {
    /// Creates the painting owner over the link its view established before
    /// the Lynx main thread began running, with the draw target it will keep.
    ///
    /// The target is built here rather than handed over later: a view is
    /// never in a state where it has run but has nowhere to put a frame.
    pub(super) async fn new(
        viewport: Viewport,
        frame_size: FrameSize,
        link: PainterLink,
        target: DrawTarget,
    ) -> Result<Self, EngineError> {
        let output = Output::build(target, frame_size).await?;
        Ok(Self::with_output(viewport, frame_size, link, output))
    }

    fn with_output(
        viewport: Viewport,
        frame_size: FrameSize,
        link: PainterLink,
        output: Output,
    ) -> Self {
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
            thread_bound: PhantomData,
        }
    }

    #[cfg(test)]
    pub(super) fn start<R: EventRequester>(
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

    #[cfg(test)]
    pub(crate) fn probe_document<T: Send + 'static>(
        &mut self,
        probe: impl FnOnce(&mut LynxDocument) -> T + Send + 'static,
    ) -> Option<T> {
        if self.detached {
            return None;
        }
        let (sender, receiver) = mpsc::channel();
        self.link.send(ToMain::Probe(Box::new(move |document| {
            let _ = sender.send(probe(document));
        })));
        receiver.recv_timeout(Duration::from_secs(10)).ok()
    }

    #[cfg(test)]
    pub(crate) fn published_frame(&mut self) -> Option<Arc<CommittedFrame>> {
        self.link.sync();
        self.link.frame().cloned()
    }

    /// The painter and the other end of its link, with no Lynx main thread
    /// started over it yet — the seam `start` spawns through, and the one a
    /// test plays the main thread's half of.
    #[cfg(test)]
    pub(super) fn with_link<R: EventRequester>(
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<R>,
        output: Output,
    ) -> (Self, MainLink<R>) {
        let (link, main) = main_link(event_requester);
        let painter = Self::with_output(viewport, frame_size, link, output);
        (painter, main)
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

    pub(super) fn note_images_changed(&self) {
        self.link.send(ToMain::NoteImagesChanged);
        self.refresh();
    }

    pub(super) fn dispatch_input(&mut self, event: InputEvent) {
        self.link.sync();
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
        self.link.sync();
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
        self.link.sync();
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
        self.link.sync();
        if !self.link.take_redraw() && !self.is_animating() {
            return Ok(());
        }
        let size = self.frame_size;
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
        let latest = self.link.frame().cloned();
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
        let Output::Window(graphics) = &mut self.output else {
            unreachable!("the window output was just checked");
        };
        if let (Some(frame), Some(key)) = (&latest, key) {
            paint_window(
                graphics,
                &self.scroll_intents,
                &mut self.composed_scene,
                frame,
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
        self.link.sync();
        let latest = self.link.frame().cloned();
        let key = latest
            .as_ref()
            .map(|frame| (frame.commit_id(), self.scroll_intents.generation));
        let animation_now = latest
            .as_ref()
            .and_then(|frame| frame.has_live_curves().then_some(now));
        match &mut self.output {
            #[cfg(test)]
            Output::None => Err(EngineError::NotOffscreen),
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
                if let (Some(frame), Some(key)) = (&latest, key) {
                    paint_window(
                        graphics,
                        &self.scroll_intents,
                        &mut self.composed_scene,
                        frame,
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
    /// Tells the Lynx main thread to stop. Its command loop returns on this
    /// message, which is why it is sent explicitly rather than left to the
    /// FIFO closing when the painter is released.
    pub(super) fn shutdown(&self) {
        self.link.send(ToMain::Shutdown);
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
        self.link.sync();
        let Some(frame) = self.link.frame().cloned() else {
            return Ok(false);
        };
        self.scroll_intents.rebase(&frame);
        self.maybe_request_refill(&frame);
        let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
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
