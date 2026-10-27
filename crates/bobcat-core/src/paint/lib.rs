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

use terminus::link::CommittedFrame;
use terminus::link::input::{InputEvent, InputKind};
use terminus::link::painter::{EmitEvent, GesturePolicy};
#[cfg(test)]
use terminus::link::painter::{FrameClock, GestureRouter, InputDecisions, ScrollIntents};
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::background::WorkerKey;
use crate::clock::ClockInstant;
use crate::link::{FramePost, InputEventPayload, Published, ToMain, ViewSeat};
#[cfg(test)]
use crate::main::tree::Viewport;
use crate::resource::ResourceFetcher;
#[cfg(not(target_arch = "wasm32"))]
use crate::view::Screenshot;
use crate::view::{DrawTarget, EngineError, FrameSize, LynxView};

/// How far a sequence may travel, in viewport CSS px, and still deliver `tap`.
///
/// Lynx reads this from the page config (`tapSlop`, default `"50px"`); wiring
/// the config key is a recorded follow-up, so today every view uses the
/// default.
pub(crate) const TAP_SLOP: f32 = 50.0;

/// Movement beyond this, in viewport CSS px, cancels a pending `longpress`.
///
/// Lynx cancels its long-press timer at the platform touch slop (nominally
/// 8dp on Android) — the same threshold the drag recognizer latches at, so on
/// scrollable content the long-press cancel and the scroll claim engage at
/// the same travel.
pub(crate) const LONG_PRESS_MOVE_SLOP: f32 = 8.0;

/// How far a pointer drag travels before it starts scrolling, in viewport
/// CSS px. The first movement subtracts the slop instead of jumping.
pub(crate) const DRAG_SLOP: f32 = 8.0;

/// How long a pointer must stay down before `longpress` fires.
///
/// Lynx's default on every platform (Android `getLongPressTimeout()`, iOS
/// `minimumPressDuration`, Harmony). The page-config override
/// (`longPressDuration`) is a recorded follow-up.
pub(crate) const LONG_PRESS_SECONDS: f64 = 0.5;

/// The event name a held sequence synthesizes.
pub(crate) const LONG_PRESS_EVENT: &str = "longpress";

/// The event name a released sequence synthesizes.
pub(crate) const TAP_EVENT: &str = "tap";

/// Lynx's gestures, synthesized per the 2026-08-21 ruling (recorded in
/// `docs/tracking/deviations.md`).
///
/// `tap` fires at release, targeted at the down-routed node, unless the
/// sequence travelled past the 50px radial [`TAP_SLOP`], the drag
/// recognizer's scroll consumed (so a boundary drag that moves nothing keeps
/// its tap — the Lynx Android behavior), a second pointer joined (Lynx
/// delivers `tap` for single-finger sequences only), or a `longpress` was
/// delivered. `longpress` fires [`LONG_PRESS_SECONDS`] into a hold that
/// stayed within [`LONG_PRESS_MOVE_SLOP`], gated on a listener existing
/// anywhere (name-level, a recorded approximation): the walk Lynx runs at that
/// instant either finds a handler or the sequence's long-press half is over.
/// A release delivers `pointerup`, `touchend`, `tap` — the order native Lynx
/// and a browser both give.
///
/// The router's touch events report web-core's `{x, y}` detail — the first
/// finger still down — and, once the last finger has lifted, the changed
/// finger's own point (native Lynx's answer; web-core yields the number `0`
/// there). A mouse produces no touch events, as in a browser and in web-core.
pub(crate) const LYNX_GESTURES: GesturePolicy = GesturePolicy {
    tap_slop: TAP_SLOP,
    long_press_move_slop: LONG_PRESS_MOVE_SLOP,
    drag_slop: DRAG_SLOP,
    long_press_seconds: LONG_PRESS_SECONDS,
    tap_event: TAP_EVENT,
    long_press_event: LONG_PRESS_EVENT,
};

/// One frame an offscreen painter has posted to its view and not yet drawn:
/// what [`Painter::post_frame`] answers, [`Painter::frame_serviced`] asks
/// about and [`Painter::draw_posted`] draws.
#[derive(Debug)]
#[must_use = "a posted frame is drawn by `Painter::draw_posted`"]
pub struct PostedFrame {
    /// The instant the post was made at, which is the instant the frame is
    /// drawn at: one reading of the clock for both halves.
    now: f64,
    /// The post's sequence number, which the view acknowledges. `None` when
    /// nothing was posted: no view is attached, or its task has gone.
    seq: Option<u64>,
}

/// A view's whole side of one link, for the tests that play that side by
/// hand rather than over a group's thread.
#[cfg(test)]
pub(crate) struct FarEnd {
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
    /// Main's handle on the seat's scroll mailbox.
    pub(crate) scroll: Arc<crate::link::ScrollMailbox>,
    /// The seat a live view holds: its command sender and its resource
    /// system. Without it here the painter's `Weak` would not upgrade, and the
    /// painter would detach itself on its first turn.
    seat: Rc<ViewSeat>,
    pub(crate) outbox: crate::link::ViewOutbox,
    #[expect(
        dead_code,
        reason = "held so the outbox's notices have somewhere to go"
    )]
    notices: mpsc::UnboundedReceiver<crate::link::ViewNotice>,
}

/// One draw target and everything that turns a view's frames into pixels in
/// it: the gesture router, the scroll intents, the composition, the frame
/// clock.
///
/// Kept on one thread by construction — the presenting painter it holds is
/// `!Send` — and that thread is the one that built it, which is therefore the
/// one that draws.
///
/// It is built before any view exists and outlives every view it shows.
/// [`Self::attach`] points it at one, [`Self::detach`] releases it, and
/// between the two everything derived from that view — the frames, the gesture
/// arena, the resolved pixels — belongs to the attachment rather than to the
/// painter. The draw target and what it holds do not: what was last drawn
/// stays on screen, and stays capturable, across both.
pub struct Painter {
    /// The presenting painter: the draw target and everything that turns
    /// adopted frames into pixels in it.
    core: terminus::link::painter::Painter,
    /// The seat this painter took on the view it observes, if it observes one.
    /// Non-owning, and the only handle there is on a live view: an upgrade
    /// that fails is the view having been released, which is therefore a fact
    /// this painter reads rather than one anybody has to tell it.
    seat: Weak<ViewSeat>,
    /// A receiver on that view's publication watch, cloned when this painter
    /// attached.
    ///
    /// Deliberately not in the seat: a released view's last frame is still the
    /// frame to draw, so the receiver has to outlive the view it came from and
    /// the poll that notices the view is gone has to read it one last time.
    frames: Option<watch::Receiver<Published>>,
    /// The last snapshot adopted from the attached view's watch: its frame is
    /// the one the presenting painter adopted, and its listener names and
    /// serviced frame post are what routing and the offscreen wait read, so
    /// one pass sees one state — and it survives the view, so a released
    /// view's last frame is still drawable and capturable.
    published: Published,
    begin_frames_sent: u64,
    /// What this painter last told the attached view's pipeline registry:
    /// the instant of the present it reported, whether it has reported the
    /// view's first paint and first contentful paint, and whether it has
    /// asked for Container Timing's stop. Cleared with everything else
    /// derived from a view.
    reported: PresentReport,
}

/// See [`Painter::reported`].
#[derive(Clone, Copy, Debug, Default)]
struct PresentReport {
    presented: Option<ClockInstant>,
    first_paint: bool,
    first_contentful_paint: bool,
    /// Whether the view's first pointer down, wheel or key down has asked
    /// the registry to end Container Timing
    /// ([`Painter::stop_container_timing_on`]).
    container_timing_stopped: bool,
}

impl std::fmt::Debug for Painter {
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
    /// Builds a painter over one draw target, on the thread that will draw
    /// into it — the only thread macOS lets a surface be created from.
    ///
    /// It observes nothing yet: [`Self::attach`] points it at a view.
    ///
    /// # Errors
    ///
    /// [`EngineError::Viewport`] if the metrics are not finite and positive
    /// or the physical target would exceed 16384 pixels on either axis, and
    /// [`EngineError::Gpu`] or [`EngineError::Render`] if the target itself
    /// cannot be built.
    pub async fn new(
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

    /// A painter with nowhere to draw, for the in-crate tests about routing,
    /// events and timers: they pay for no GPU device, and need no executor to
    /// build one.
    #[cfg(test)]
    pub(crate) fn without_output(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        Self::with_core(terminus::link::painter::Painter::without_output(
            width,
            height,
            device_pixel_ratio,
            LYNX_GESTURES,
        ))
    }

    /// A painter attached to a view nobody serves, so a test can play that
    /// view's whole side of the link by hand.
    ///
    /// Built rather than attached, deliberately: `attach` drops everything
    /// derived from a previous view and rebases the frame clock, and these
    /// tests want the painter exactly as it was constructed.
    ///
    /// The seat is bound from the start — its metrics watch already holds
    /// this painter's viewport — because the view these tests play is one a
    /// painter is already watching, and an unbound one would park its first
    /// flush.
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
                    // A token of its own: this far end plays the view, and
                    // nothing here is ever released.
                    tokio_util::sync::CancellationToken::new(),
                    // No fetcher, so nothing to probe.
                    None,
                    None,
                    crate::link::detached_base(),
                    registry,
                ),
                notices: notice_receiver,
            },
        )
    }

    /// Starts observing `view`: its frames, its listener names, and the
    /// pixels its host has loaded.
    ///
    /// Everything derived from whatever came before is dropped first — the
    /// published snapshot, the scroll intents, the gesture arena, the resolved
    /// pixels, and what the target holds: its compose key. Commit ids restart
    /// at one per document, so a key kept across the change would make the new
    /// page's first frame look already drawn.
    ///
    /// The painter's metrics win: attaching writes them into the view's seat,
    /// so a view built at one size and shown at another is resized rather
    /// than showing a frame the target cannot present. That write is also the
    /// **binding**: a view publishes no frame and reports no
    /// [`EngineEvent::ScriptFinished`](crate::EngineEvent::ScriptFinished)
    /// before it, and its first `__FlushElementTree` parks until it happens.
    /// Detaching does not undo it — only the first binding is waited for.
    ///
    /// # Errors
    ///
    /// [`EngineError::PainterAttached`] if this painter already observes a
    /// live view, or `view` already has an interactive painter of its own —
    /// there is exactly one per view. [`EngineError::Render`] if this
    /// painter's draw target has already failed, so reuse is refused rather
    /// than silently drawing nothing.
    pub fn attach<F: ResourceFetcher + 'static>(
        &mut self,
        view: &LynxView<F>,
    ) -> Result<(), EngineError> {
        if self.core.has_failed() {
            return Err(EngineError::Render(
                "the painter's draw target failed".to_owned(),
            ));
        }
        // A seat whose view is gone is not an attachment, only one this painter
        // has not taken a turn to notice — `is_attached` reads the view's
        // liveness rather than a bit somebody set — which is what lets a host
        // replace a view without pumping in between.
        if self.is_attached() {
            return Err(EngineError::PainterAttached);
        }
        // This is the only place a view's seat is downgraded, so a view has at
        // most one weak handle on its seat: the painter observing it.
        debug_assert!(
            Rc::weak_count(view.seat()) <= 1,
            "only `Painter::attach` downgrades a view's seat"
        );
        if Rc::weak_count(view.seat()) > 0 {
            return Err(EngineError::PainterAttached);
        }
        let frames = view.frames();
        // A frame post a previous attachment sent and main has not
        // acknowledged yet could otherwise satisfy this attachment's first
        // sequence one frame early. Starting past whatever has been
        // serviced costs at most one extra turn on the next tick.
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

    /// Writes this painter's metrics into the attached view's watch, which is
    /// what binds the view.
    ///
    /// The first write is the binding: it releases a `__FlushElementTree`
    /// parked on it, and the document lays out at these metrics from then on.
    /// Every later write is an ordinary resize. [`Self::detach`] deliberately
    /// writes nothing — only the first binding is waited for, so a view whose
    /// painter went keeps the last metrics and never parks again.
    fn bind_metrics(&self) {
        if let Some(seat) = self.seat.upgrade() {
            seat.metrics.send_replace(Some(self.core.viewport()));
        }
    }

    /// Stops observing the view, if it was observing one.
    ///
    /// The draw target is untouched: what it last rendered stays on screen
    /// and stays capturable, which is what makes detaching usable while the
    /// next page loads. Nothing is said to the view — a painter cannot end
    /// one — and nothing is cancelled.
    pub fn detach(&mut self) {
        if self.frames.take().is_none() {
            return;
        }
        self.seat = Weak::new();
        self.forget_view();
    }

    /// Whether this painter is observing a view.
    ///
    /// A view that has been released is not one: its seat is gone with it, and
    /// nothing had to tell this painter so.
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.seat.strong_count() > 0
    }

    /// Whether this painter's draw target has failed. A failed painter draws
    /// nothing further and refuses to attach.
    #[must_use]
    pub const fn has_failed(&self) -> bool {
        self.core.has_failed()
    }

    /// Drops everything derived from a view: what it published, where it was
    /// scrolled to, the gesture in progress, and the pixels resolved out of its
    /// host's store.
    ///
    /// Not what the target holds — that is the presenting painter's
    /// `forget_target`, and it survives a detach, because so do the pixels on
    /// screen.
    ///
    /// What the intents had not published yet is dropped with them: nothing
    /// on main acts on a container coming to rest yet (a `scrollend` on
    /// detach is the scroll events' to decide).
    fn forget_view(&mut self) {
        self.published = Published::default();
        self.reported = PresentReport::default();
        self.core.forget_view();
    }

    /// Tells the attached view's pipeline registry about a present it has not
    /// heard of: the frame's commit id and its compose and present instants,
    /// which complete the pipelines waiting on that commit, and the view's
    /// first paint and first contentful paint once each exists. Each BTS
    /// worker the completion owes entries to is sent one
    /// `PerformanceReady`.
    ///
    /// A comparison of one instant when nothing new was presented; with a
    /// new present and no pipeline waiting, two atomic accesses inside the
    /// registry and no lock.
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

    /// Hands the presenting painter's Container Timing reports to the
    /// attached view's registry, which makes the `container` entries the
    /// Rust embedder's next `pump` takes when it observes them. One
    /// `is_empty` on a view with no `containertiming` root, whose painter
    /// never reports any; a detached painter's reports are dropped.
    fn report_container_paints(&mut self) {
        let paints = self.core.take_container_paints();
        if !paints.is_empty()
            && let Some(seat) = self.seat.upgrade()
        {
            seat.registry.note_container_paints(&paints);
        }
    }

    /// Container Timing stops at the first scroll or input the window
    /// dispatches; for a view that is the first pointer down, wheel or key
    /// down the host routes to it, whatever it hits and whether or not a
    /// listener hears it. Asks the registry once per view, which the main
    /// thread's next commit acts on; every later event reads one `bool`.
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

    /// Sends each BTS worker in `owed` one `PerformanceReady`.
    fn notify_owed(seat: &ViewSeat, owed: Vec<WorkerKey>) {
        if !owed.is_empty() {
            let demand = seat.frame_demand.borrow();
            for key in owed {
                demand.notify_performance(key);
            }
        }
    }

    /// Sets the presenting painter's presentation timing from the attached
    /// view's registry before a draw
    /// ([`crate::performance::PipelineRegistry::wants_presentation_timing`]:
    /// a record waits for a present, the document detects content, or the
    /// view's first paint is not recorded yet), and answers the seat and
    /// the commit the draw will present when the switch is off, which
    /// [`Self::report_untimed_present`] tells the registry of. A detached
    /// painter leaves the switch as it is.
    fn sync_presentation_timing(&mut self) -> Option<(Rc<ViewSeat>, u64)> {
        let seat = self.seat.upgrade()?;
        let timed = seat.registry.wants_presentation_timing();
        self.core.set_presentation_timing(timed);
        if timed {
            return None;
        }
        let commit_id = self.frame()?.commit_id();
        Some((seat, commit_id))
    }

    /// After a draw made with presentation timing off: the commit it
    /// presented, which a flush may have bound a record to since the switch
    /// was read — that record completes now rather than waiting for a
    /// present that nothing may ever ask for. Two atomic accesses when no
    /// record waits.
    fn report_untimed_present(untimed: Option<(Rc<ViewSeat>, u64)>) {
        if let Some((seat, commit_id)) = untimed {
            let owed = seat.registry.note_untimed_present(commit_id);
            Self::notify_owed(&seat, owed);
        }
    }

    /// Adopts whatever the attached view has published since the last look —
    /// the frame and the pixels it draws, together — then notices a view that
    /// has gone.
    ///
    /// Unconditional, and first, in every call that observes anything: a
    /// painter with no window still has to see the frame it will capture, and
    /// an occluded one still has to see the commit it owes a redraw for.
    ///
    /// A commit is adopted only if its pixels can be read in the same step.
    /// The two are one fact — a frame indexes its store's bitmaps by draw
    /// order, so a frame from one commit over another commit's table draws the
    /// wrong images — and a released view takes its store with it. So the last
    /// commit of a view that has gone is adopted if this painter read its
    /// pixels while the view was still there, and left behind if it did not.
    ///
    /// The snapshot is taken rather than only read when the watch reports a
    /// change: a completed `changed()` has already marked the value seen, so
    /// a flag alone would skip exactly the state an offscreen wait was woken
    /// for. Never `Receiver::has_changed()` either — that errors once the
    /// sender is gone, and the last frame a view published before its task
    /// ended is still the frame this painter must draw.
    ///
    /// **May block.** A store is allowed — required, in fact — to restore a
    /// bitmap it evicted, and adopting a commit is the read that asks it to.
    /// Every poll on the drawing path runs before that path acquires a
    /// swap-chain image, so a restore cannot stall the chain under vsync.
    fn poll_link(&mut self) {
        let Some(frames) = self.frames.as_mut() else {
            return;
        };
        let published = frames.borrow_and_update();
        let latest = published.clone();
        drop(published);
        // One upgrade, for both halves of the seat: a commit's pixels are read
        // out of the view's store through the same handle whose loss means the
        // view is gone, so the two can never disagree.
        let seat = self.seat.upgrade();
        let store = seat.as_ref().map(|seat| seat.images.as_ref());
        if !self.core.adopt(latest.frame.clone(), store) {
            // The view is gone with its store, so this commit's pixels can no
            // longer be read. It is not adopted; the painter keeps the last
            // commit it did read, whole.
            self.release_seat();
            return;
        }
        self.published = latest;
        // Losing the view's seat is the view being released, which is the one
        // end a painter has to notice — and it is noticed after the adoption
        // above, so a commit published in the release turn is still taken. It
        // keeps what it adopted, what it composed and what it drew: the last
        // frame stays on screen.
        if seat.is_none() {
            self.release_seat();
        }
    }

    /// Lets go of a view that has been released: the dangling weak seat, and
    /// the watch whose publisher went with it.
    fn release_seat(&mut self) {
        self.seat = Weak::new();
        self.frames = None;
    }

    /// Sends one command to the attached view's task, publishing this pass's
    /// scroll state first. A detached painter, or one whose view has ended,
    /// says nothing; it goes on showing what it last drew.
    fn send(&mut self, now: f64, command: ToMain) {
        self.publish_scroll(now, None);
        if let Some(seat) = self.seat.upgrade() {
            let _ = seat.commands.send(command);
        }
    }

    /// Posts what this pass changed into the view's mailbox, with `frame`
    /// when the pass asks main for one, and sends the marker when the post
    /// is the first since main's last take. Every send of a pass runs this
    /// first, so a command never reaches main ahead of the scroll state of
    /// its own pass, and every pass ends with it.
    ///
    /// Records the pass's clock either way; takes no lock when there is
    /// nothing to post.
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

    /// The newest committed frame this painter has adopted.
    fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.core.frame()
    }

    /// Whether the attached view's scripts asked for a display frame
    /// (`requestAnimationFrame` on any of its realms). The presenting painter
    /// asks it only when the adopted frame's own animations do not already
    /// answer, the order [`Self::is_animating`]'s terms have always had —
    /// answering it sweeps the workers that have gone.
    fn frame_demand_pending(&self) -> bool {
        self.seat
            .upgrade()
            .is_some_and(|seat| seat.frame_demand.borrow_mut().is_pending())
    }

    /// Whether the engine owes the timeline another frame, as of the last
    /// pass that polled the view — a `pump`, an offscreen frame, or an input. That is
    /// when a host asks: after answering the wakeup that carried the frame.
    ///
    /// A detached painter owes nothing: there is no timeline to be behind.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.is_attached() && self.core.is_animating(|| self.frame_demand_pending())
    }

    /// Routes one normalized OS input event against the frame this painter
    /// last read.
    ///
    /// A detached painter routes nothing: it has no frame to hit-test against
    /// and nobody to deliver to, so the event is dropped before the gesture
    /// router sees it rather than opening a sequence that can never close.
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
        // A key moves no scroll: its pass has no scroll state of its own to
        // post.
        if !matches!(event.kind, InputKind::Key { .. }) {
            self.publish_scroll(at, None);
        }
    }

    /// Sends the events one pass decided, in order, to the view: each as a
    /// [`ToMain::DispatchEvent`], and only those the view has a listener for
    /// anywhere — the painter's replica of the listener names, as of this
    /// pass.
    ///
    /// `at_seconds` is the clock reading of the pass that produced them — an
    /// input event's arrival, or the tick's own `now` — and is what every
    /// event the pass dispatches reports as its `timestamp`. It is one
    /// reading for the whole pass, so a due `longpress` flushed ahead of the
    /// event that found it and the `tap` synthesized after it are stamped
    /// with that event's arrival, which is the moment they all belong to.
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

    /// Executes one pass's decisions in order: the presenting painter applies
    /// the scrolls, and the events they decided are sent
    /// ([`Self::send_events`]).
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

    /// Runs the gesture clock over the frame this painter has already
    /// adopted, without polling for a newer one: the drawing path calls this
    /// while it holds a swap-chain image, and [`Self::poll_link`] may block
    /// on a store restoring an evicted bitmap.
    fn tick_gestures(&mut self, now: f64) {
        let mut emits = Vec::new();
        self.core.tick_gestures(now, &self.published, &mut emits);
        self.send_events(emits, now);
        // The flings and bounce backs advance on the same clock reading,
        // after any decision this tick made about them.
        self.core.tick_scroll(now);
    }

    /// Applies new device metrics, if they moved at all.
    ///
    /// The size is validated first, so a target the painter could not render
    /// is refused before anything else has seen it. Resize belongs to the
    /// painter alone — it is the side that owns the surface — so a detached
    /// painter records the new metrics and imposes them on whichever view it
    /// attaches to next.
    ///
    /// # Errors
    ///
    /// [`EngineError::Viewport`] if the metrics are not finite and positive,
    /// or if the physical target would exceed 16384 pixels on either axis.
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), EngineError> {
        if self.core.resize(width, height, device_pixel_ratio)? {
            // A detached painter has no seat to write, so it only records the
            // new metrics and imposes them on whichever view it attaches to
            // next.
            self.bind_metrics();
        }
        Ok(())
    }

    /// The physical pixel size of this painter's draw target.
    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.core.frame_size()
    }

    /// Asks for a frame nothing else would have asked for.
    ///
    /// It wakes nobody: every caller is on this thread, inside a call the
    /// host is already making, so the turn that host is in is the turn that
    /// answers it.
    pub fn refresh(&self) {
        self.core.refresh();
    }

    /// Reports whether the window is visible. A window nobody can see draws
    /// nothing, and un-occluding asks again for the frame that was held back.
    pub fn set_occluded(&mut self, occluded: bool) {
        self.core.set_occluded(occluded);
    }

    /// Runs one painter turn: adopt whatever the view published, and draw the
    /// frame it owes.
    ///
    /// This is where a windowed painter draws, so a host calls it at the
    /// point in its own turn where a wait for the display is acceptable, and
    /// once per turn. It asks the host for nothing: servicing the resource
    /// protocol is [`crate::LynxView::pump`]'s, and a host takes both turns.
    ///
    /// # Errors
    ///
    /// [`EngineError::Render`] or [`EngineError::Gpu`] once, when the draw
    /// target fails. There is no recovering a lost surface, so afterwards
    /// this answers `Ok` and draws nothing.
    pub fn pump(&mut self) -> Result<(), EngineError> {
        self.poll_link();
        self.report_presentation();
        if self.core.has_failed() {
            return Ok(());
        }
        // A draw that fails marks the target failed itself
        // (`terminus::link::painter::Painter::has_failed`), so this is reported
        // once.
        self.draw()
    }

    /// Deliver an actual display vsync reported by the host. Pending requests
    /// participate in `owes_frame`; ordinary pumps and repaints deliver no rAF.
    pub fn vsync(&mut self) {
        self.poll_link();
        if !self.core.has_failed() && !self.core.is_occluded() {
            self.deliver_vsync(self.core.now_seconds());
        }
    }

    /// Delivers the script frame requests, after this pass's scroll state:
    /// HTML runs the scroll steps before the animation frame callbacks.
    fn deliver_vsync(&mut self, now: f64) {
        self.publish_scroll(now, None);
        if let Some(seat) = self.seat.upgrade() {
            seat.frame_demand
                .borrow_mut()
                .dispatch(now * 1000.0, &seat.commands);
        }
    }

    /// Posts a frame request for main when one is due — something animates
    /// on main, a curve passed its end, or `always` — beside this pass's
    /// scroll state, and answers its sequence number: the number the view
    /// acknowledges ([`Self::frame_serviced`]), whether or not this post had
    /// to send a marker. `None` when nothing was posted or the view's task is
    /// gone.
    pub(super) fn begin_frame(&mut self, now: f64, always: bool) -> Option<u64> {
        let main_ticks_due = self
            .frame()
            .is_some_and(|frame| frame.needs_main_ticks() || frame.animation_boundary_passed(now));
        let seat = self.seat.upgrade()?;
        if seat.commands.is_closed() {
            return None;
        }
        // Behind every command the seat's sender has already sent: main
        // applies the post only once it has applied them. A native module's
        // answer to the MTS realm is not sent through it, so the post waits
        // for none.
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

    /// One window frame: the swap-chain image first, then the gesture clock,
    /// the scroll rebase and the frame post at the frame's own clock reading,
    /// then the compose, render and present.
    fn draw(&mut self) -> Result<(), EngineError> {
        if !self.core.presents_to_window() || self.core.is_occluded() {
            return Ok(());
        }
        if !self.core.take_redraw() && !self.is_animating() {
            return Ok(());
        }
        // Nothing committed yet, or no image this frame: the presenting
        // painter keeps the frame owed, and the host takes it at its next
        // display frame like any other.
        let Some(window_frame) = self.core.begin_draw()? else {
            return Ok(());
        };
        let now = window_frame.now();
        self.tick_gestures(now);
        // Before the post, so a commit adopted this turn cannot re-snap an
        // offset after its state went out. The post is the pass's last
        // publish: nothing below moves an offset.
        self.core.rebase_scroll(now);
        let _ = self.begin_frame(now, false);
        let untimed = self.sync_presentation_timing();
        self.core.finish_draw(window_frame)?;
        self.report_presentation();
        Self::report_untimed_present(untimed);
        Ok(())
    }

    /// Reads back what this painter last rendered.
    ///
    /// It asks the view's host for nothing: what it composes is whatever that
    /// view has already published, and the pixels come out of the store the
    /// view's own `pump` filled.
    ///
    /// # Errors
    ///
    /// [`EngineError::Render`] if nothing has been rendered to read back, and
    /// [`EngineError::Gpu`] or [`EngineError::Render`] if bringing the target
    /// up to the newest frame, or the read itself, fails.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        // Refused before anything is polled, so a painter with no target says
        // so rather than reporting the empty target a compose would leave.
        #[cfg(test)]
        if !self.core.has_target() {
            return Err(EngineError::Render(
                "this painter has no draw target to capture".to_owned(),
            ));
        }
        let now = self.core.now_seconds();
        self.poll_link();
        // The newest frame is brought to the target first, so a capture is of
        // what this painter would draw rather than of whatever it last drew.
        Ok(self.core.capture(now)?)
    }

    /// Whether this painter has a frame to put on its window.
    ///
    /// A running animation, a swap chain that had no image to give, and a
    /// frame something asked for that no turn has produced yet are one
    /// answer, because a host serves them all the same way: at its own next
    /// display frame. No delay is named here — the display's clock belongs to
    /// the embedder, and this is the whole of what the engine has to say
    /// about when to read it.
    ///
    /// Always false for a window nobody can see, a target that failed, a
    /// painter observing no view, and one that presents to no window at all:
    /// an offscreen painter's frames are the host's to ask for through
    /// [`Self::post_frame`].
    #[must_use]
    pub fn owes_frame(&self) -> bool {
        self.is_attached() && self.core.owes_frame(|| self.frame_demand_pending())
    }

    /// The newest committed frame, for the seams outside this module that
    /// read one.
    #[cfg(test)]
    pub(crate) fn published_frame(&mut self) -> Option<Arc<CommittedFrame>> {
        self.poll_link();
        self.frame().cloned()
    }

    /// Starts one offscreen frame: delivers a synthetic vsync to outstanding
    /// script-frame requests, then posts the frame to the view, and answers
    /// what was posted.
    ///
    /// An offscreen painter has no display to pace against, so a host that
    /// wants a frame carrying everything it sent before asks for one: this
    /// posts it, [`Self::frame_serviced`] reads whether the view has serviced
    /// it, and [`Self::draw_posted`] draws. None of the three waits. The wait
    /// between the post and the draw is the host's own, in its own loop,
    /// because the view may need the host during it: a script that adopts a
    /// stylesheet, requires a module or waits a `Future` parks its job on an
    /// answer [`crate::LynxView::pump`] gives, and the frame post is queued
    /// behind that job. A host therefore takes the view's turn while it
    /// waits:
    ///
    /// ```text
    /// let posted = painter.post_frame()?;
    /// while !painter.frame_serviced(&posted) && before the host's own deadline {
    ///     for event in view.pump() { … }
    ///     wait for the host's wakeup
    /// }
    /// painter.draw_posted(posted, false)?;
    /// ```
    ///
    /// The view wakes the host through its `EventRequester` both when it asks
    /// the host for something and when it services a post, so that wakeup is
    /// what the loop waits on.
    ///
    /// # Errors
    ///
    /// [`EngineError::NotOffscreen`] if this painter presents into a window —
    /// its frames come from [`Self::pump`], on the host's own clock, and it
    /// never waits for the view at all.
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

    /// Whether the view has serviced `posted`, adopting everything the view
    /// has published on the way. Never blocks.
    ///
    /// `true` as well when nothing will ever service it: nothing was posted,
    /// or the view was released, or its tasks have ended — a boot that failed
    /// ends them — so a host waiting on this is not left waiting on a view
    /// that has gone.
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

    /// Draws the frame [`Self::post_frame`] started, answering whether it
    /// drew: whatever the view has published by now, at the instant the post
    /// was made.
    ///
    /// A host calls it once [`Self::frame_serviced`] answers `true`, and may
    /// call it without that — its own deadline passed — in which case this
    /// draws what was already published and the post is serviced later.
    ///
    /// # Errors
    ///
    /// [`EngineError::NotOffscreen`] if this painter presents into a window,
    /// and [`EngineError::Gpu`] if the render fails.
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
        // The host's wait may have adopted a newer commit. What this rebase
        // changes is the pass's last publish; no send of the pass follows it.
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

    /// One whole offscreen frame with the wait in the middle blocked on, for
    /// the crate's tests: post, [`Self::wait_begin_frame`], draw. A host has
    /// no such call, because the wait is where it takes its view's turn.
    #[cfg(test)]
    pub(crate) fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        let posted = self.post_frame()?;
        if let Some(seq) = posted.seq {
            let _ = self.wait_begin_frame(seq, Duration::from_secs(10));
        }
        self.draw_posted(posted, force)
    }

    /// Waits for the attached view to acknowledge a particular frame post,
    /// adopting everything it publishes on the way.
    ///
    /// A test's wait and nobody else's: it blocks the calling thread on
    /// `bobcat-main`, which no entry point a host has does. The tests that
    /// play a view by hand, or boot one whose scripts ask their host for
    /// nothing while a frame is outstanding, have no turn to take meanwhile.
    /// It ends early on a view whose tasks have gone, because nothing will
    /// service that sequence number then.
    #[cfg(test)]
    pub(crate) fn wait_begin_frame(&mut self, seq: u64, timeout: Duration) -> bool {
        let deadline = ClockInstant::now() + timeout;
        self.poll_link();
        while self.published.begin_frame_serviced < seq && ClockInstant::now() < deadline {
            // A released view, or one whose tasks have ended and closed the
            // channel: nothing will service this sequence number, so waiting
            // the deadline out would be the host's own thread spent on nothing.
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
                // The deadline passed, or the view's publisher is gone.
                None | Some(Err(_)) => break,
                Some(Ok(())) => {}
            }
            self.poll_link();
        }
        // Once more on the way out, for the exits that leave the loop without
        // one: the deadline passing, and a far end that has gone. Either can
        // land in the same instant as the acknowledgement this was waiting
        // for, and a view that published one before its tasks ended has
        // still serviced it.
        self.poll_link();
        self.published.begin_frame_serviced >= seq
    }
}

/// The presenting painter's insides, for the tests that pin its clock, drive
/// its intents by hand, or read its router.
#[cfg(test)]
impl Painter {
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
