//! The presenter thread's object: input routing, scrolling, composition,
//! and every draw target the view has.
//!
//! Everything here runs on one engine-owned thread. It answers the host
//! through its [`PainterLink`] and the Lynx main thread through its
//! [`PresenterLink`], and nothing it owns — a surface, a scene buffer, a
//! gesture arena — is ever touched from anywhere else.

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

#[cfg(not(target_arch = "wasm32"))]
use super::Screenshot;
use super::graphics::{FrameAcquisition, WindowGraphics};
use super::{ComposeKey, EngineError, EngineEvent, EventRequester, FrameSize};
use crate::clock::FrameClock;
use crate::gesture::{EmitEvent, GestureRouter, InputDecision, InputDecisions, RouterHost};
use crate::link::{MainLink, PainterLink, PresenterLink, ToMain, ToPainter, link};
use crate::tree::Viewport;

const BEGIN_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the presenter waits before asking a swap chain that had nothing
/// to give. Half a display refresh: soon enough that a transient miss costs
/// no visible frame, slow enough that a window the compositor has stopped
/// serving does not cost a core.
const SWAP_CHAIN_RETRY: Duration = Duration::from_millis(8);

/// Where a view's pixels go: nowhere yet, an offscreen GPU target, or a
/// window's presentation stack.
pub(super) enum Output {
    None,
    Offscreen(Box<Headless>),
    Window(Box<WindowGraphics>),
}

/// The presenting half of a running view, on the thread that owns it.
///
/// Kept on that thread by construction — the `Rc` marker makes the whole
/// struct `!Send`, so the only way it reaches the presenter is to be built
/// there.
pub(crate) struct Painter {
    // Keep first: dropping the link closes the sole command sender, which
    // wakes the Lynx main thread before any state it may still refer to is
    // released.
    pub(super) link: PresenterLink,
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
    link: &'a PresenterLink,
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
    /// The painter and the other end of its link, with no Lynx main thread
    /// started over it yet — the seam `start` spawns through, and the one a
    /// test plays the main thread's half of.
    pub(super) fn with_link<R: EventRequester>(
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<R>,
    ) -> (Self, MainLink<R>) {
        let (presenter, main) = link(event_requester);
        let painter = Self {
            link: presenter,
            #[cfg(test)]
            detached: true,
            viewport,
            frame_size,
            output: Output::None,
            occluded: false,
            render_failed: false,
            gesture: GestureRouter::default(),
            clock: FrameClock::new(),
            scroll_intents: ScrollIntents::default(),
            composed: None,
            composed_scene: Scene::new(),
            refill_requested_for: None,
            thread_bound: PhantomData,
        };
        (painter, main)
    }

    /// Whether the engine owes the timeline another frame, as of the last
    /// pass that drained the link — a `pump`, a `draw`, or an input. That is
    /// when a host asks: after answering the wakeup that carried the frame.
    #[must_use]
    pub(super) fn is_animating(&self) -> bool {
        self.link
            .frame()
            .is_some_and(|frame| frame.animations_active())
            || self.gesture.needs_frame()
    }

    fn note_images_changed(&self) {
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

    /// Applies the host's new metrics. The host validated them and computed
    /// the frame size, so nothing here can fail.
    fn resize(&mut self, viewport: Viewport, frame_size: FrameSize) {
        self.link.send(ToMain::Resize {
            width: viewport.width,
            height: viewport.height,
            device_pixel_ratio: viewport.device_pixel_ratio,
        });
        self.viewport = viewport;
        self.frame_size = frame_size;
        self.refresh();
    }

    pub(super) fn refresh(&self) {
        self.link.mark_redraw();
    }

    /// A window nobody can see draws nothing, and un-occluding asks again
    /// for the frame that was held back.
    fn set_occluded(&mut self, occluded: bool) {
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

    /// Takes over the presentation stack the host built on its own thread.
    fn attach_graphics(&mut self, graphics: Box<WindowGraphics>) {
        self.output = Output::Window(graphics);
        self.render_failed = false;
        self.refresh();
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
                // that out, so the frame stays owed and the presenter asks
                // again on its own short delay rather than in a spin.
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
    pub(super) fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        let gpu = Headless::new().map_err(|error| EngineError::Gpu(error.to_string()))?;
        self.output = Output::Offscreen(Box::new(gpu));
        self.composed = None;
        self.render_failed = false;
        Ok(())
    }

    /// Applies one host command, answering whether the presenter goes on.
    ///
    /// A command the host is blocked on hands back everything the turn has
    /// produced *before* it answers, so a `tick` that returns has already
    /// put its events and its animation state in the host's hands.
    pub(super) fn apply<H: EventRequester>(
        &mut self,
        command: ToPainter,
        host: &PainterLink<H>,
    ) -> bool {
        match command {
            // The turn this opened will sync; there is nothing else to do.
            #[cfg(not(target_arch = "wasm32"))]
            ToPainter::MainChanged => {}
            ToPainter::Input(event) => self.dispatch_input(event),
            ToPainter::Resize {
                viewport,
                frame_size,
            } => self.resize(viewport, frame_size),
            ToPainter::Occluded(occluded) => self.set_occluded(occluded),
            ToPainter::Refresh => self.refresh(),
            ToPainter::Attach(graphics) => self.attach_graphics(graphics),
            ToPainter::AttachOffscreen(reply) => {
                let answer = self.attach_offscreen();
                self.hand_back(host);
                let _ = reply.send(answer);
            }
            ToPainter::Tick { force, reply } => {
                let answer = self.tick(force);
                self.hand_back(host);
                let _ = reply.send(answer);
            }
            #[cfg(not(target_arch = "wasm32"))]
            ToPainter::Capture(reply) => {
                let answer = self.capture();
                self.hand_back(host);
                let _ = reply.send(answer);
            }
            ToPainter::NoteImagesChanged => self.note_images_changed(),
            ToPainter::Shutdown => return false,
        }
        true
    }

    /// Ends one turn: produce the frame it owes, hand the host what the
    /// realm had to say, and publish whether the timeline wants another.
    ///
    /// In that order deliberately. Drawing first means the pixels a fatal
    /// script error left behind reach the screen on the turn that reports
    /// it, with nobody left to ask for another frame.
    ///
    /// A draw that fails is reported once. There is no recovering a lost
    /// surface, and the loop would otherwise report the same failure on
    /// every turn for as long as the host takes to notice the first.
    pub(super) fn serve<H: EventRequester>(&mut self, host: &PainterLink<H>) {
        if !self.render_failed
            && let Err(error) = self.draw()
        {
            self.render_failed = true;
            host.report(EngineEvent::RenderFailed(error));
        }
        self.hand_back(host);
    }

    /// Hands the host everything this turn produced.
    fn hand_back<H: EventRequester>(&mut self, host: &PainterLink<H>) {
        for event in self.pump() {
            host.report(event);
        }
        host.set_animating(self.is_animating());
    }

    /// When the presenter owes itself another turn, and how soon.
    ///
    /// `None` parks until something arrives. Zero is a running animation:
    /// the swap chain's `AutoVsync` acquire inside the next draw is the
    /// pace, and asking for the turn immediately is what keeps the frames
    /// coming. Anything else is a swap chain that had no image to give —
    /// which it answers without waiting for vsync, so the retry needs a
    /// delay of its own or it becomes a spin.
    ///
    /// Only ever a visible, working window. An offscreen target has no
    /// display to keep up with: its frames are the host's to ask for.
    pub(super) fn next_turn(&self) -> Option<Duration> {
        if self.render_failed || self.occluded || !matches!(self.output, Output::Window(_)) {
            return None;
        }
        if self.is_animating() {
            return Some(Duration::ZERO);
        }
        self.link.redraw_owed().then_some(SWAP_CHAIN_RETRY)
    }

    pub(super) fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        if !matches!(self.output, Output::Offscreen(_)) {
            return Err(EngineError::NoDrawTarget);
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
