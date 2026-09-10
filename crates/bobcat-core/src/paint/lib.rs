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
use std::sync::Arc;
use std::time::Duration;

use dom::input::{InputEvent, InputKind};
use dom::render::gpu::Headless;
use dom::scroll::ScrollAxes;
use dom::vello::Scene;
use dom::vello::peniko::Color;
use dom::{CommittedFrame, HitTarget, NodeId, Vector2D};
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, watch};

use self::gesture::{EmitEvent, GestureRouter, InputDecision, InputDecisions, RouterHost};
pub use self::graphics::WindowTarget;
use self::graphics::{FrameAcquisition, WindowGraphics};
use crate::clock::ClockInstant;
use crate::link::{Published, ToMain, ViewCancel, ViewNotice, ViewOutbox, block_on_deadline};
use crate::main::tree::Viewport;
use crate::resource::{SourceCompletion, SourceRequest};
#[cfg(not(target_arch = "wasm32"))]
use crate::view::Screenshot;
use crate::view::{ComposeKey, DrawTarget, EngineError, EngineEvent, FrameSize};

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

/// The painting end of the view's one link — to the task serving it on the
/// Lynx main thread — including the snapshot it routes and draws against
/// without touching that thread.
pub(crate) struct PainterLink {
    commands: mpsc::UnboundedSender<ToMain>,
    notices: mpsc::UnboundedReceiver<ViewNotice>,
    frames: watch::Receiver<Published>,
    /// The last snapshot adopted from the watch. Everything routing and
    /// drawing read comes from here, so one pass sees one state.
    published: Published,
    cancel: ViewCancel,
    events: Vec<EngineEvent>,
    begin_frames_sent: u64,
    redraw_pending: Cell<bool>,
    /// Sources the document met and wants named. Buffered here because
    /// asking for them needs the host's resource system, which the painter
    /// owns rather than the link.
    image_requests: Vec<Arc<str>>,
    source_requests: Vec<(SourceRequest, SourceCompletion)>,
    /// Whether a fatal lifecycle event has arrived. Nothing further is
    /// dispatched to the host's resource system after one.
    failed: bool,
}

impl PainterLink {
    pub(crate) fn new(
        commands: mpsc::UnboundedSender<ToMain>,
        notices: mpsc::UnboundedReceiver<ViewNotice>,
        frames: watch::Receiver<Published>,
        cancel: ViewCancel,
    ) -> Self {
        Self {
            commands,
            notices,
            frames,
            published: Published::default(),
            cancel,
            events: Vec::new(),
            begin_frames_sent: 0,
            redraw_pending: Cell::new(false),
            image_requests: Vec::new(),
            source_requests: Vec::new(),
            failed: false,
        }
    }

    /// Sends one command. A closed channel is a view whose task has ended;
    /// the painter goes on showing what it last published.
    pub(crate) fn send(&self, command: ToMain) {
        let _ = self.commands.send(command);
    }

    /// Applies everything that has arrived, then adopts the newest published
    /// state. However many frames were committed, the watch is read once.
    pub(crate) fn sync(&mut self) {
        while let Ok(notice) = self.notices.try_recv() {
            self.apply(notice);
        }
        let _ = self.adopt();
    }

    fn apply(&mut self, notice: ViewNotice) {
        match notice {
            ViewNotice::Engine(event) => {
                if matches!(
                    event,
                    EngineEvent::StartupFailed(_) | EngineEvent::ScriptRunError(_)
                ) {
                    self.failed = true;
                    self.cancel.cancel();
                }
                self.events.push(event);
            }
            ViewNotice::RequestImages(sources) => self.image_requests.extend(sources),
            ViewNotice::RequestSource {
                request,
                completion,
            } => self.source_requests.push((request, completion)),
        }
    }

    /// Takes the newest published state, and says whether it had moved
    /// since this link last looked.
    ///
    /// The snapshot is taken unconditionally rather than only when the watch
    /// reports a change: a completed `changed()` has already marked the value
    /// seen, so the flag alone would skip exactly the state an offscreen wait
    /// was woken for.
    ///
    /// Never `Receiver::has_changed()` either: that reports an error once the
    /// sender is gone, and the last frame a view published before its task
    /// ended is still the frame this painter must draw.
    fn adopt(&mut self) -> bool {
        let latest = self.frames.borrow_and_update();
        let changed = latest.has_changed();
        let published = latest.clone();
        drop(latest);
        if published.commit() != self.published.commit() {
            self.redraw_pending.set(true);
        }
        self.published = published;
        changed
    }

    fn take_image_requests(&mut self) -> Vec<Arc<str>> {
        std::mem::take(&mut self.image_requests)
    }

    pub(crate) fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.published.frame.as_ref()
    }

    pub(crate) fn has_listener(&self, name: &str) -> bool {
        self.published.listeners.contains(name)
    }

    pub(crate) fn take_events(&mut self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.events)
    }

    /// Cancels this view: no source completion still in a host's hands may
    /// answer after this.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    #[cfg(test)]
    pub(crate) const fn view_cancel(&self) -> &ViewCancel {
        &self.cancel
    }

    /// Every listener name the view has published, for a test that asserts
    /// the whole set rather than one membership.
    #[cfg(test)]
    pub(crate) fn listener_names(&self) -> Vec<Arc<str>> {
        self.published.listeners.iter().cloned().collect()
    }

    /// The next notice the view sent, for a test that plays the host itself
    /// rather than draining through a painter turn.
    #[cfg(test)]
    pub(crate) fn take_notice(&mut self) -> Option<ViewNotice> {
        self.notices.try_recv().ok()
    }

    /// Whether the published state moved since this was last asked, which
    /// for a test driving the listener index alone is whether an edge
    /// crossed.
    #[cfg(test)]
    pub(crate) fn take_published_edge(&mut self) -> bool {
        self.adopt()
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

    /// Waits for a particular main-thread animation round while applying
    /// everything that precedes its acknowledgement.
    ///
    /// The one blocking wait a host's own thread makes on `bobcat-main`, and
    /// `tick` — offscreen only — is the one call that reaches it. It ends
    /// early on a fatal event, because nothing will service the round after
    /// one, and on a task that has gone, because nothing will service it at
    /// all.
    pub(crate) fn wait_begin_frame(&mut self, seq: u64, timeout: Duration) -> bool {
        let deadline = ClockInstant::now() + timeout;
        self.sync();
        while self.published.begin_frame_serviced < seq
            && !self.failed
            && !self.commands.is_closed()
            && ClockInstant::now() < deadline
        {
            // Destructured so the two halves can be borrowed at once inside
            // one future.
            let Self {
                frames, notices, ..
            } = self;
            let woken = block_on_deadline(
                async {
                    tokio::select! {
                        published = frames.changed() => Wake::Published(published.is_ok()),
                        notice = notices.recv() => Wake::Notice(notice),
                    }
                },
                deadline,
            );
            match woken {
                // The deadline passed, or the far end is gone.
                None | Some(Wake::Notice(None) | Wake::Published(false)) => break,
                // Applied rather than left on the channel: a notice consumed
                // here is one this view will never see again.
                Some(Wake::Notice(Some(notice))) => self.apply(notice),
                Some(Wake::Published(true)) => {}
            }
            self.sync();
        }
        // Once more on the way out, for the exits that leave the loop without
        // one: the deadline passing, and a far end that has gone. Either can
        // land in the same instant as the acknowledgement this was waiting
        // for, and a view that published one before its task ended has still
        // serviced the round.
        self.sync();
        self.published.begin_frame_serviced >= seq
    }
}

/// What ended one turn of an offscreen frame wait.
enum Wake {
    /// The published state moved; `false` if its sender is gone.
    Published(bool),
    /// One notice, or the end of the channel.
    Notice(Option<ViewNotice>),
}

impl fmt::Debug for PainterLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PainterLink")
            .field("listener_names", &self.published.listeners.len())
            .field("begin_frames_sent", &self.begin_frames_sent)
            .finish_non_exhaustive()
    }
}

/// Both ends of one view's link, for a caller that is itself the far end: the
/// crate's benchmarks, and the tests that drive a document in place rather
/// than over a group's thread.
pub(crate) fn detached_link(
    requester: Arc<dyn crate::view::EventRequester>,
) -> (PainterLink, ViewOutbox, mpsc::UnboundedReceiver<ToMain>) {
    let cancel = ViewCancel::default();
    let (commands, command_receiver) = mpsc::unbounded_channel();
    let (notices, notice_receiver) = mpsc::unbounded_channel();
    let (frames, frame_receiver) = watch::channel(Published::default());
    (
        PainterLink::new(commands, notice_receiver, frame_receiver, cancel.clone()),
        ViewOutbox::new(notices, frames, requester, cancel),
        command_receiver,
    )
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
            #[cfg(test)]
            DrawTarget::None => Ok(Self::None),
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
/// The painter the in-crate tests that play the far end of a link build: no
/// test here is about the host's resource system, so they all share the one
/// that answers nothing.
#[cfg(test)]
pub(crate) type TestPainter = Painter<crate::resource::NeverAnswers>;

pub(crate) struct Painter<F> {
    // Keep first: dropping the link closes the sole command sender, which
    // ends this view's task before any state it may still refer to is
    // released.
    pub(super) link: PainterLink,
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
    /// Ends this view. Cancellation first, so a host still holding one of its
    /// source completions sees it before anything is released; the goodbye
    /// itself is the command sender dropping with this painter.
    ///
    /// Teardown knows nothing about the store, so it stays reachable for any
    /// `F` — which is what lets `Drop` run without the trait bound.
    pub(super) fn shutdown(&self) {
        self.link.cancel();
        // Close the sink before the store drops: a loader still in flight
        // must find it detached rather than queue into a dead view.
        self.images.detach();
    }

    /// One command, for the seams outside this module that have one to send.
    #[cfg(test)]
    pub(crate) fn send(&self, command: ToMain) {
        self.link.send(command);
    }

    /// This view's cancellation flag, for a test that holds a source
    /// completion of its own.
    #[cfg(test)]
    pub(crate) const fn view_cancel(&self) -> &ViewCancel {
        self.link.view_cancel()
    }
}

/// The far end of a link, for the tests that play the main thread's whole
/// side of one.
#[cfg(test)]
pub(crate) struct DetachedEnds {
    pub(crate) outbox: ViewOutbox,
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
}

/// The test constructor pins the fetcher: no test that plays the far end of a
/// link is about the host's resource system, so they all build over the one
/// that answers nothing.
#[cfg(test)]
impl TestPainter {
    /// A painter with every seam built but no view task: the other end of its
    /// link is handed back so a test can play that task's whole side of it.
    pub(super) fn detached(
        viewport: Viewport,
        frame_size: FrameSize,
        requester: Arc<dyn crate::view::EventRequester>,
    ) -> (Self, DetachedEnds) {
        let (link, outbox, commands) = detached_link(requester);
        let painter = Self::with_output(viewport, frame_size, link, Output::None, |_reports| {
            crate::resource::NeverAnswers
        });
        (painter, DetachedEnds { outbox, commands })
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
        // A view that failed or was released asks its host for nothing more:
        // every completion still queued here is dropped, which is what tells
        // whoever was awaiting it that no source is coming.
        for (request, completion) in std::mem::take(&mut self.link.source_requests) {
            if !self.link.failed && !completion.is_cancelled() {
                self.images.store().request_source(request, completion);
            }
        }
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

    /// The newest committed frame, for the seams outside this module that
    /// read one.
    #[cfg(test)]
    pub(crate) fn published_frame(&mut self) -> Option<Arc<CommittedFrame>> {
        self.sync();
        self.link.frame().cloned()
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
