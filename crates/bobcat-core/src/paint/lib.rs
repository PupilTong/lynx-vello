//! The painter: input routing, scrolling, composition, and one draw target.
//!
//! A painter is a standalone object an embedder builds on the thread that
//! will draw, and points at a view by attaching to it. Everything here runs
//! on that thread, inside the calls the embedder makes, and nothing it owns —
//! a surface, a scene buffer, a gesture arena — is ever touched from anywhere
//! else. [`Painter`] is `!Send` by construction, which is what makes the
//! constructing thread the painting thread for its whole life.
//!
//! Everything that turns frames into pixels — the gesture router, the scroll
//! intents, the composition, the frame clock and the draw target — is
//! `terminus`'s presenting painter ([`terminus::link::painter::Painter`]). This
//! module is its link to one view: what it holds of a view is a receiver on
//! the view's publication watch and a `Weak` on the view's [`ViewSeat`].
//! Neither owns anything: a painter observes a view; it does not keep one
//! alive, cannot end one, and asks its host for nothing — servicing the
//! resource protocol is [`crate::LynxView::pump`]'s alone. It adopts what the
//! view publishes into the presenting painter, sends the events that painter
//! decides as [`ToMain::DispatchEvent`] commands — only those some listener
//! in the document is registered for — posts its scroll offsets and frame
//! requests into the view's mailbox, and delivers the script frame requests.
//!
//! Lynx's gesture vocabulary and thresholds are the policy the presenting
//! painter is built with ([`LYNX_GESTURES`]).
//!
//! Its scroll physics are fixed in `terminus` rather than passed in, and
//! they are lynx-ui's `useBounce`
//! (`packages/lynx-ui-scroll-view/src/hooks/useBounce.tsx`), so a page that
//! bounces through `overscroll-behavior: contain-bounce` and one that bounces
//! through that hook's transforms move the same way: the rubber band's
//! coefficient is its `rubberC` (`UIKit`'s), a fling's decay past a boundary
//! its `flingDeceleratingRate`, the bounce back's stiffness its `beta`, and a
//! fling past a boundary stretches raw up to one scrollport, as its overshoot
//! does. A stretched fling ends once its velocity is under one physical
//! pixel, lynx-ui's rule, so the bounce back starts promptly. lynx-ui leaves
//! the in-range fling to the native scroller and names no rate of its own, so
//! that decay is `UIKit`'s normal deceleration rate; and it springs only from
//! rest, so the limit on a glide's initial velocity, which keeps the spring
//! from crossing its target, is the engine's own.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod animation_tests;
pub(crate) mod canvas_pictures;
#[cfg(test)]
mod event_loop_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use terminus::link::input::{InputEvent, InputKind};
use terminus::link::painter::{EmitEvent, GesturePolicy};
#[cfg(test)]
use terminus::link::painter::{FrameClock, GestureRouter, InputDecisions, ScrollIntents};
use terminus::link::{CommittedFrame, FrameWriter};
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::watch;

pub(crate) use self::canvas_pictures::CanvasPictures;
use self::canvas_pictures::ViewImages;
use crate::background::WorkerKey;
use crate::clock::ClockInstant;
use crate::link::{FramePost, InputEventPayload, Published, ToMain, ViewSeat};
#[cfg(test)]
use crate::main::tree::Viewport;
use crate::resource::ResourceFetcher;
#[cfg(not(target_arch = "wasm32"))]
use crate::view::Screenshot;
use crate::view::{DrawTarget, EngineError, FrameSize, LynxView};

pub(crate) const TAP_SLOP: f32 = 50.0;

pub(crate) const LONG_PRESS_MOVE_SLOP: f32 = 8.0;

pub(crate) const DRAG_SLOP: f32 = 8.0;

pub(crate) const LONG_PRESS_SECONDS: f64 = 0.5;

pub(crate) const LONG_PRESS_EVENT: &str = "longpress";

pub(crate) const TAP_EVENT: &str = "tap";

pub(crate) const LYNX_GESTURES: GesturePolicy = GesturePolicy {
    tap_slop: TAP_SLOP,
    long_press_move_slop: LONG_PRESS_MOVE_SLOP,
    drag_slop: DRAG_SLOP,
    long_press_seconds: LONG_PRESS_SECONDS,
    tap_event: TAP_EVENT,
    long_press_event: LONG_PRESS_EVENT,
};

#[derive(Debug)]
#[must_use = "a posted frame is drawn by `Painter::draw_posted`"]
pub struct PostedFrame {
    now: f64,
    seq: Option<u64>,
}

#[cfg(test)]
pub(crate) struct FarEnd {
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
    pub(crate) scroll: Arc<crate::link::ScrollMailbox>,
    seat: Rc<ViewSeat>,
    pub(crate) outbox: crate::link::ViewOutbox,
    #[expect(
        dead_code,
        reason = "held so the outbox's notices have somewhere to go"
    )]
    notices: mpsc::UnboundedReceiver<crate::link::ViewNotice>,
}

pub struct Painter<P = std::convert::Infallible> {
    core: terminus::link::painter::Painter,
    seat: Weak<ViewSeat<P>>,
    frames: Option<watch::Receiver<Published>>,
    published: Published,
    begin_frames_sent: u64,
    reported: PresentReport,
}

#[derive(Clone, Copy, Debug, Default)]
struct PresentReport {
    presented: Option<ClockInstant>,
    first_paint: bool,
    first_contentful_paint: bool,
    container_timing_stopped: bool,
}

impl<P: FrameWriter + 'static> std::fmt::Debug for Painter<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Painter")
            .field("viewport", &self.core.viewport())
            .field("frame_size", &self.core.frame_size())
            .field("attached", &self.is_attached())
            .finish_non_exhaustive()
    }
}

impl Painter {
    pub async fn new(
        target: DrawTarget,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, EngineError> {
        Self::with_platform_frames(target, width, height, device_pixel_ratio).await
    }

    #[cfg(test)]
    pub(crate) fn without_output(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        Self::with_core(terminus::link::painter::Painter::without_output(
            width,
            height,
            device_pixel_ratio,
            LYNX_GESTURES,
        ))
    }

    #[cfg(test)]
    pub(super) fn detached(
        width: f32,
        height: f32,
        requester: Arc<dyn crate::view::EventRequester>,
    ) -> (Self, FarEnd) {
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let (notices, notice_receiver) = mpsc::unbounded_channel();
        let (frames, frame_receiver) = watch::channel(Published::default());
        let viewport = Viewport::new(width, height);
        let scroll = Arc::new(crate::link::ScrollMailbox::default());
        let seat = Rc::new(ViewSeat {
            frame_demand: RefCell::default(),
            commands: crate::link::CommandSender::new(commands),
            metrics: watch::channel(Some(viewport)).0,
            images: Rc::new(terminus::link::NoImages),
            canvases: Arc::default(),
            scroll: Arc::clone(&scroll),
            registry: crate::performance::PipelineRegistry::detached(),
        });
        let mut painter = Self::without_output(width, height, 1.0);
        painter.seat = Rc::downgrade(&seat);
        painter.frames = Some(frame_receiver);
        let registry = Arc::clone(&seat.registry);
        (
            painter,
            FarEnd {
                commands: command_receiver,
                scroll,
                seat,
                outbox: crate::link::ViewOutbox::new(
                    notices,
                    frames,
                    requester,
                    tokio_util::sync::CancellationToken::new(),
                    None,
                    None,
                    crate::link::detached_base(),
                    registry,
                ),
                notices: notice_receiver,
            },
        )
    }
}

impl<P: FrameWriter + 'static> Painter<P> {
    pub async fn with_platform_frames(
        target: DrawTarget,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, EngineError> {
        let core = terminus::link::painter::Painter::new(
            target,
            width,
            height,
            device_pixel_ratio,
            LYNX_GESTURES,
        )
        .await?;
        Ok(Self::with_core(core))
    }

    fn with_core(core: terminus::link::painter::Painter) -> Self {
        Self {
            core,
            seat: Weak::new(),
            frames: None,
            published: Published::default(),
            begin_frames_sent: 0,
            reported: PresentReport::default(),
        }
    }

    pub fn attach<F: ResourceFetcher<PlatformFrame = P> + 'static>(
        &mut self,
        view: &LynxView<F>,
    ) -> Result<(), EngineError> {
        if self.core.has_failed() {
            return Err(EngineError::Render(
                "the painter's draw target failed".to_owned(),
            ));
        }
        if self.is_attached() {
            return Err(EngineError::PainterAttached);
        }
        debug_assert!(
            Rc::weak_count(view.seat()) <= 1,
            "only `Painter::attach` downgrades a view's seat"
        );
        if Rc::weak_count(view.seat()) > 0 {
            return Err(EngineError::PainterAttached);
        }
        let frames = view.frames();
        self.begin_frames_sent = self
            .begin_frames_sent
            .max(frames.borrow().begin_frame_serviced);
        self.seat = Rc::downgrade(view.seat());
        self.frames = Some(frames);
        self.forget_view();
        self.core.forget_target();
        self.core.rebase_clock(view.timeline_epoch());
        self.bind_metrics();
        self.refresh();
        Ok(())
    }

    fn bind_metrics(&self) {
        if let Some(seat) = self.seat.upgrade() {
            seat.metrics.send_replace(Some(self.core.viewport()));
        }
    }

    pub fn detach(&mut self) {
        if self.frames.take().is_none() {
            return;
        }
        self.seat = Weak::new();
        self.forget_view();
    }

    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.seat.strong_count() > 0
    }

    #[must_use]
    pub const fn has_failed(&self) -> bool {
        self.core.has_failed()
    }

    fn forget_view(&mut self) {
        self.published = Published::default();
        self.reported = PresentReport::default();
        self.core.forget_view();
    }

    fn report_presentation(&mut self) {
        self.report_container_paints();
        let Some(frame) = self.core.last_presented() else {
            return;
        };
        if self.reported.presented == Some(frame.presented) {
            return;
        }
        let Some(seat) = self.seat.upgrade() else {
            return;
        };
        self.reported.presented = Some(frame.presented);
        let first_paint = self.core.first_paint();
        let first_contentful_paint = self.core.first_contentful_paint();
        if (!self.reported.first_paint && first_paint.is_some())
            || (!self.reported.first_contentful_paint && first_contentful_paint.is_some())
        {
            self.reported.first_paint |= first_paint.is_some();
            self.reported.first_contentful_paint |= first_contentful_paint.is_some();
            let owed = seat
                .registry
                .note_paint(first_paint, first_contentful_paint);
            Self::notify_owed(&seat, owed);
        }
        let owed = seat.registry.note_presented(frame);
        Self::notify_owed(&seat, owed);
    }

    fn report_container_paints(&mut self) {
        let paints = self.core.take_container_paints();
        if !paints.is_empty()
            && let Some(seat) = self.seat.upgrade()
        {
            seat.registry.note_container_paints(&paints);
        }
    }

    fn stop_container_timing_on(&mut self, event: &InputEvent) {
        use terminus::link::input::{KeyPhase, PointerPhase};
        if self.reported.container_timing_stopped {
            return;
        }
        let interacts = match &event.kind {
            InputKind::Pointer { phase, .. } => *phase == PointerPhase::Down,
            InputKind::Wheel { .. } => true,
            InputKind::Key { phase, .. } => *phase == KeyPhase::Down,
            _ => false,
        };
        if interacts && let Some(seat) = self.seat.upgrade() {
            self.reported.container_timing_stopped = true;
            seat.registry.request_container_stop();
        }
    }

    fn notify_owed(seat: &ViewSeat<P>, owed: Vec<WorkerKey>) {
        if !owed.is_empty() {
            let demand = seat.frame_demand.borrow();
            for key in owed {
                demand.notify_performance(key);
            }
        }
    }

    fn sync_presentation_timing(&mut self) -> Option<(Rc<ViewSeat<P>>, u64)> {
        let seat = self.seat.upgrade()?;
        let timed = seat.registry.wants_presentation_timing();
        self.core.set_presentation_timing(timed);
        if timed {
            return None;
        }
        let commit_id = self.frame()?.commit_id();
        Some((seat, commit_id))
    }

    fn report_untimed_present(untimed: Option<(Rc<ViewSeat<P>>, u64)>) {
        if let Some((seat, commit_id)) = untimed {
            let owed = seat.registry.note_untimed_present(commit_id);
            Self::notify_owed(&seat, owed);
        }
    }

    fn poll_link(&mut self) {
        let Some(frames) = self.frames.as_mut() else {
            return;
        };
        let published = frames.borrow_and_update();
        let latest = published.clone();
        drop(published);
        let seat = self.seat.upgrade();
        let store = seat.as_ref().map(|seat| ViewImages {
            canvases: &seat.canvases,
            store: seat.images.as_ref(),
        });
        if !self.core.adopt(latest.frame.clone(), store.as_ref()) {
            self.release_seat();
            return;
        }
        self.published = latest;
        if seat.is_none() {
            self.release_seat();
        }
    }

    fn release_seat(&mut self) {
        self.seat = Weak::new();
        self.frames = None;
    }

    fn send(&mut self, now: f64, command: ToMain) {
        self.publish_scroll(now, None);
        if let Some(seat) = self.seat.upgrade() {
            let _ = seat.commands.send(command);
        }
    }

    fn publish_scroll(&mut self, now: f64, frame: Option<FramePost>) {
        let Some(seat) = self.seat.upgrade() else {
            self.core.discard_changed();
            return;
        };
        seat.scroll.set_clock(now);
        if !self.core.scroll_changed() && frame.is_none() {
            return;
        }
        let entries = self.core.drain_changed(now);
        if seat.scroll.post(entries, frame) {
            let _ = seat.commands.send(ToMain::Posted);
        }
    }

    fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.core.frame()
    }

    fn frame_demand_pending(&self) -> bool {
        self.seat
            .upgrade()
            .is_some_and(|seat| seat.frame_demand.borrow_mut().is_pending())
    }

    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.is_attached() && self.core.is_animating(|| self.frame_demand_pending())
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "the embedder hands the event over; a key event's strings are shared, not copied"
    )]
    pub fn dispatch_input(&mut self, event: InputEvent) {
        if !self.is_attached() {
            return;
        }
        self.poll_link();
        self.stop_container_timing_on(&event);
        let mut emits = Vec::new();
        let at = self.core.route_input(&event, &self.published, &mut emits);
        self.send_events(emits, at);
        if !matches!(event.kind, InputKind::Key { .. }) {
            self.publish_scroll(at, None);
        }
    }

    fn send_events(&mut self, emits: Vec<EmitEvent>, at_seconds: f64) {
        let timestamp = at_seconds * 1000.0;
        for event in emits {
            if !self.published.listeners.contains(event.name) {
                continue;
            }
            self.send(
                at_seconds,
                ToMain::DispatchEvent {
                    target: event.target,
                    name: event.name,
                    payload: InputEventPayload {
                        detail: event.detail,
                        timestamp,
                    },
                    release: event.release,
                },
            );
        }
    }

    #[cfg(test)]
    pub(super) fn execute_decisions(
        &mut self,
        decisions: &mut InputDecisions,
        published: Option<&CommittedFrame>,
        at_seconds: f64,
    ) {
        let mut emits = Vec::new();
        self.core
            .apply_decisions(decisions, published, at_seconds, &mut emits);
        self.send_events(emits, at_seconds);
    }

    pub(super) fn service_gesture_clock(&mut self, now: f64) {
        self.poll_link();
        self.tick_gestures(now);
    }

    fn tick_gestures(&mut self, now: f64) {
        let mut emits = Vec::new();
        self.core.tick_gestures(now, &self.published, &mut emits);
        self.send_events(emits, now);
        self.core.tick_scroll(now);
    }

    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), EngineError> {
        if self.core.resize(width, height, device_pixel_ratio)? {
            self.bind_metrics();
        }
        Ok(())
    }

    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.core.frame_size()
    }

    pub fn refresh(&self) {
        self.core.refresh();
    }

    pub fn set_occluded(&mut self, occluded: bool) {
        self.core.set_occluded(occluded);
    }

    pub fn pump(&mut self) -> Result<(), EngineError> {
        #[cfg(target_arch = "wasm32")]
        terminus::link::render::canvas::drain();
        self.poll_link();
        self.report_presentation();
        if self.core.has_failed() {
            return Ok(());
        }
        self.draw()
    }

    pub fn vsync(&mut self) {
        self.poll_link();
        if !self.core.has_failed() && !self.core.is_occluded() {
            self.deliver_vsync(self.core.now_seconds());
        }
    }

    fn deliver_vsync(&mut self, now: f64) {
        self.publish_scroll(now, None);
        if let Some(seat) = self.seat.upgrade() {
            seat.frame_demand
                .borrow_mut()
                .dispatch(now * 1000.0, &seat.commands);
        }
    }

    pub(super) fn begin_frame(&mut self, now: f64, always: bool) -> Option<u64> {
        let main_ticks_due = self
            .frame()
            .is_some_and(|frame| frame.needs_main_ticks() || frame.animation_boundary_passed(now));
        let seat = self.seat.upgrade()?;
        if seat.commands.is_closed() {
            return None;
        }
        let fence = seat.commands.sent();
        let post = (main_ticks_due || always).then(|| {
            self.begin_frames_sent += 1;
            FramePost {
                now,
                seq: self.begin_frames_sent,
                fence,
            }
        });
        self.publish_scroll(now, post);
        post.map(|post| post.seq)
    }

    fn draw(&mut self) -> Result<(), EngineError> {
        if !self.core.presents_to_window() || self.core.is_occluded() {
            return Ok(());
        }
        if !self.core.take_redraw() && !self.is_animating() {
            return Ok(());
        }
        let Some(window_frame) = self.core.begin_draw()? else {
            return Ok(());
        };
        let now = window_frame.now();
        self.tick_gestures(now);
        self.core.rebase_scroll(now);
        let _ = self.begin_frame(now, false);
        let untimed = self.sync_presentation_timing();
        self.core.finish_draw(window_frame)?;
        self.report_presentation();
        Self::report_untimed_present(untimed);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        #[cfg(test)]
        if !self.core.has_target() {
            return Err(EngineError::Render(
                "this painter has no draw target to capture".to_owned(),
            ));
        }
        let now = self.core.now_seconds();
        self.poll_link();
        Ok(self.core.capture(now)?)
    }

    pub fn awaits_pictures(&mut self) -> bool {
        self.poll_link();
        self.core.awaits_pictures()
    }

    #[must_use]
    pub fn owes_frame(&self) -> bool {
        self.is_attached() && self.core.owes_frame(|| self.frame_demand_pending())
    }

    #[cfg(test)]
    pub(crate) fn published_frame(&mut self) -> Option<Arc<CommittedFrame>> {
        self.poll_link();
        self.frame().cloned()
    }

    pub fn post_frame(&mut self) -> Result<PostedFrame, EngineError> {
        if !self.core.is_offscreen() {
            return Err(EngineError::NotOffscreen);
        }
        let now = self.core.now_seconds();
        self.report_presentation();
        self.service_gesture_clock(now);
        self.core.rebase_scroll(now);
        self.deliver_vsync(now);
        let seq = self.begin_frame(now, true);
        Ok(PostedFrame { now, seq })
    }

    pub fn frame_serviced(&mut self, posted: &PostedFrame) -> bool {
        self.poll_link();
        let Some(seq) = posted.seq else {
            return true;
        };
        self.published.begin_frame_serviced >= seq
            || self
                .seat
                .upgrade()
                .is_none_or(|seat| seat.commands.is_closed())
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "a posted frame is drawn once, and taking it is what says so"
    )]
    pub fn draw_posted(&mut self, posted: PostedFrame, force: bool) -> Result<bool, EngineError> {
        if !self.core.is_offscreen() {
            return Err(EngineError::NotOffscreen);
        }
        let PostedFrame { now, .. } = posted;
        self.poll_link();
        if self.frame().is_none() {
            self.publish_scroll(now, None);
            return Ok(false);
        }
        self.core.rebase_scroll(now);
        self.publish_scroll(now, None);
        let untimed = self.sync_presentation_timing();
        let drew = self.core.draw_offscreen(now, force)?;
        self.report_presentation();
        if drew {
            Self::report_untimed_present(untimed);
        }
        Ok(drew)
    }

    #[cfg(test)]
    pub(crate) fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        let posted = self.post_frame()?;
        if let Some(seq) = posted.seq {
            let _ = self.wait_begin_frame(seq, Duration::from_secs(10));
        }
        self.draw_posted(posted, force)
    }

    #[cfg(test)]
    pub(crate) fn wait_begin_frame(&mut self, seq: u64, timeout: Duration) -> bool {
        let deadline = ClockInstant::now() + timeout;
        self.poll_link();
        while self.published.begin_frame_serviced < seq && ClockInstant::now() < deadline {
            if self
                .seat
                .upgrade()
                .is_none_or(|seat| seat.commands.is_closed())
            {
                break;
            }
            let Some(frames) = self.frames.as_mut() else {
                break;
            };
            match crate::link::block_on_deadline(frames.changed(), deadline) {
                None | Some(Err(_)) => break,
                Some(Ok(())) => {}
            }
            self.poll_link();
        }
        self.poll_link();
        self.published.begin_frame_serviced >= seq
    }
}

#[cfg(test)]
impl<P: FrameWriter + 'static> Painter<P> {
    pub(crate) fn clock_mut(&mut self) -> &mut FrameClock {
        self.core.clock_mut()
    }

    pub(crate) fn now_seconds(&self) -> f64 {
        self.core.now_seconds()
    }

    pub(crate) fn scroll_intents(&self) -> &ScrollIntents {
        self.core.scroll_intents()
    }

    pub(crate) fn scroll_intents_mut(&mut self) -> &mut ScrollIntents {
        self.core.scroll_intents_mut()
    }

    pub(crate) fn gesture(&self) -> &GestureRouter {
        self.core.gesture()
    }

    pub(crate) fn take_redraw(&self) -> bool {
        self.core.take_redraw()
    }

    pub(crate) fn redraw_pending(&self) -> bool {
        self.core.redraw_pending()
    }

    pub(crate) fn set_frame_size(&mut self, size: FrameSize) {
        self.core.set_frame_size(size);
    }
}
