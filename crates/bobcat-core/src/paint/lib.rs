//! The painter: input routing, scrolling, composition, and one draw target.
//!
//! A painter is a standalone object an embedder builds on the thread that
//! will draw, and points at a view by attaching to it. Everything here runs
//! on that thread, inside the calls the embedder makes, and nothing it owns —
//! a surface, a scene buffer, a gesture arena — is ever touched from anywhere
//! else. [`Painter`] is `!Send` by construction, which is what makes the
//! constructing thread the painting thread for its whole life.
//!
//! What it holds of a view is a receiver on the view's publication watch and
//! a `Weak` on the view's [`ViewSeat`]. Neither owns anything: a painter
//! observes a view; it does not keep one alive, cannot end one, and asks its
//! host for nothing — servicing the resource protocol is
//! [`crate::LynxView::pump`]'s alone.

mod gesture;
mod graphics;
pub(crate) mod images;
mod inertia;
mod motion;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod animation_tests;
#[cfg(test)]
mod event_loop_tests;
#[cfg(test)]
mod tests;

use std::cell::Cell;
#[cfg(test)]
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use dom::input::{InputEvent, InputKind, PointerId};
use dom::render::gpu::Headless;
use dom::scroll::{ChainLink, ScrollAxes, drive_chain, settle_offset};
use dom::vello::Scene;
use dom::vello::peniko::{Color, ImageData};
use dom::{CommittedFrame, HitTarget, NodeId, Vector2D};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::watch;

pub(crate) use self::gesture::RouterHost;
use self::gesture::{GestureRouter, InputDecision, InputDecisions};
pub use self::graphics::WindowTarget;
use self::graphics::{FrameAcquisition, WindowGraphics};
use self::inertia::{Axis, BounceBack, ChainOutcome, DragTrack, Fling};
use self::motion::{Motion, resolve_elastic_step, stretch_of, unwind_stretch};
use crate::clock::ClockInstant;
use crate::link::{InputEventPayload, Published, ToMain, ViewSeat, block_on_deadline};
use crate::main::tree::Viewport;
use crate::resource::ResourceFetcher;
#[cfg(not(target_arch = "wasm32"))]
use crate::view::Screenshot;
use crate::view::{ComposeKey, DrawTarget, EngineError, FrameSize, LynxView};

const BEGIN_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// The painter's monotonic animation timeline. Its epoch is the construction
/// of whichever view it is attached to, and one reading is shared by every
/// operation in a frame.
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

    /// Moves the epoch onto a view's own timeline.
    ///
    /// A painter attaching to a fresh view reproduces the old semantics
    /// exactly — the two instants are the same moment. A painter re-attached
    /// to a document that has already been running keeps reading the time
    /// that document's animations were started against, rather than restarting
    /// them at whatever the painter's own age happens to be.
    pub(crate) fn rebase(&mut self, epoch: ClockInstant) {
        self.epoch = epoch;
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

/// A view's whole side of one link, for the tests that play that side by
/// hand rather than over a group's thread.
#[cfg(test)]
pub(crate) struct FarEnd {
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
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

/// Where a painter's pixels go: a window's presentation stack, or a texture
/// it owns and nothing displays. It is built with the painter and is the one
/// it has for its whole life, whatever views come and go past it.
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

    /// Gives up the frame a target that is itself the only copy of one holds.
    ///
    /// What a painter does when it points at a different document. A window
    /// keeps its surface, because what is on screen is the previous page's
    /// last frame and it stays there until the next one is presented. An
    /// offscreen target is given up instead and rebuilt on the next render:
    /// its texture is the only place a frame exists, so keeping it would hand
    /// a reader the previous document's pixels as this one's.
    fn forget(&mut self) {
        match self {
            #[cfg(test)]
            Self::None => {}
            Self::Offscreen(gpu) => gpu.forget(),
            // A window keeps its surface and its last presented frame, but
            // the filter bakes still have to forget which *frame* they belong
            // to: commit ids restart at one per document.
            Self::Window(graphics) => graphics.forget_filters(),
        }
    }

    /// Bakes `frame`'s `filter: blur()` groups and `backdrop-filter`
    /// elements into `out`, index-parallel with the frame's filter entries.
    ///
    /// The table is copied out rather than borrowed because the very next
    /// step needs the target mutably again to render; an `ImageData` is a
    /// blob handle plus four fields, and `out` keeps its capacity across
    /// frames, so the copy allocates nothing in the steady state.
    ///
    /// # Errors
    ///
    /// [`EngineError::Gpu`] if a bake render fails.
    fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        offset_of: &dyn Fn(&dom::ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
        out: &mut Vec<Option<ImageData>>,
    ) -> Result<(), EngineError> {
        out.clear();
        let baked = match self {
            #[cfg(test)]
            // A painter with nowhere to draw bakes nothing, so every entry
            // falls back to replaying raw — which is what a routing test
            // wants: no device.
            Self::None => return Ok(()),
            Self::Offscreen(gpu) => gpu
                .prepare_filters(frame, images, offset_of, scroll_generation, animation_now)
                .map_err(|error| EngineError::Gpu(error.to_string()))?,
            Self::Window(graphics) => graphics.prepare_filters(
                frame,
                images,
                offset_of,
                scroll_generation,
                animation_now,
            )?,
        };
        out.extend(baked.iter().cloned());
        Ok(())
    }

    /// Renders one composed scene, drawing `images`, into this target at
    /// `size`.
    ///
    /// A window renders into the texture it re-presents rather than into the
    /// swap-chain image, which is what lets re-exposure present the same frame
    /// again with a blit alone. Neither kind waits for the submitted work here:
    /// a window presents, and an offscreen target's readback polls the device
    /// itself — only [`Painter::tick`], which renders offscreen and reads
    /// nothing back, waits.
    ///
    /// The scene's bitmaps are named because a target tracks what its
    /// renderer's image atlas still holds; see
    /// [`dom::render::gpu::AtlasResidency`].
    fn render(
        &mut self,
        scene: &Scene,
        images: &[Option<ImageData>],
        size: FrameSize,
    ) -> Result<(), EngineError> {
        match self {
            #[cfg(test)]
            Self::None => Ok(()),
            Self::Offscreen(gpu) => gpu
                .render_frame(scene, images, size.width, size.height, Color::WHITE)
                .map_err(|error| EngineError::Gpu(error.to_string())),
            Self::Window(graphics) => graphics.render_to_target(scene, images, size),
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

/// One draw target and everything that turns a view's frames into pixels in
/// it: the gesture router, the scroll intents, the composition, the frame
/// clock.
///
/// Kept on one thread by construction — the `Rc` marker makes the whole
/// struct `!Send` — and that thread is the one that built it, which is
/// therefore the one that draws.
///
/// It is built before any view exists and outlives every view it shows.
/// [`Self::attach`] points it at one, [`Self::detach`] releases it, and
/// between the two everything derived from that view — the frames, the gesture
/// arena, the resolved pixels — belongs to the attachment rather than to the
/// painter. The draw target and what it holds do not: what was last drawn
/// stays on screen, and stays capturable, across both.
pub struct Painter {
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
    viewport: Viewport,
    frame_size: FrameSize,
    output: Output,
    /// A window nobody can see draws nothing; the frame it owes stays owed.
    occluded: bool,
    /// A draw target that failed once cannot be reached again: it is reported
    /// once, and nothing tries to paint it afterwards. Painter-lifetime state:
    /// neither attaching nor detaching clears it, because neither replaces the
    /// target that failed.
    render_failed: bool,
    /// The last snapshot adopted from the attached view's watch. Everything
    /// routing and drawing read comes from here, so one pass sees one state —
    /// and it survives the view, so a released view's last frame is still
    /// drawable and capturable.
    published: Published,
    redraw_pending: Cell<bool>,
    begin_frames_sent: u64,
    pub(super) gesture: GestureRouter,
    pub(super) clock: FrameClock,
    pub(super) scroll_intents: ScrollIntents,
    /// What this painter's draw target holds: the compose key it was rendered
    /// from, at the size it was rendered for. The one record of that — a
    /// target has no second one of its own — so it says whether a frame may be
    /// skipped and whether there is anything to capture.
    ///
    /// The target's, not the attachment's: what was drawn stays drawn across a
    /// detach, and [`Painter::attach`] is what clears it.
    composed: Option<(ComposeKey, FrameSize)>,
    composed_scene: Scene,
    /// The `filter: blur()` textures of the frame being composed, copied out
    /// of the target's bake cache. Kept here so its capacity outlives a
    /// frame.
    composed_filters: Vec<Option<ImageData>>,
    refill_requested_for: Option<u64>,
    /// The pixels this commit draws, read out of the attached view's store.
    images: images::PainterImages,
    thread_bound: PhantomData<Rc<()>>,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Painter")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .field("attached", &self.is_attached())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub(super) struct ScrollIntents {
    /// Each container's live offset where it differs from the committed
    /// one. In range on every axis, except that a `contain-bounce` axis
    /// may stand up to one scrollport past either edge while it is
    /// stretched — a drag past the boundary, a fling's overshoot, or a
    /// bounce back on its way home.
    pub(super) offsets: FxHashMap<NodeId, Vector2D<f32>>,
    rebased_commit: Option<u64>,
    generation: u64,
    /// Where each drag found each container it moved: the offset before the
    /// drag's first step on it, keyed by the pointer and the container. The
    /// drag's end settles every entry of its pointer onto a snap position
    /// from there ([`Self::settle`]), and while an entry exists the
    /// container is held — a commit does not re-snap it under the finger.
    /// A fling keeps its drag's entries until it ends.
    gesture_origins: FxHashMap<(PointerId, NodeId), Vector2D<f32>>,
    /// The flings in flight; see [`inertia`].
    flings: SmallVec<[Fling; 1]>,
    /// The stretched axes springing back; see [`inertia`].
    bounce_backs: SmallVec<[BounceBack; 2]>,
    /// The drags in progress, for their release velocity; see [`inertia`].
    drags: FxHashMap<PointerId, DragTrack>,
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
                clamp_intent_axis(
                    offset.x,
                    slot.max_offset.x,
                    slot.scrollport.width,
                    slot.bounce.x,
                ),
                clamp_intent_axis(
                    offset.y,
                    slot.max_offset.y,
                    slot.scrollport.height,
                    slot.bounce.y,
                ),
            );
            *offset != slot.offset
        });
        self.gesture_origins
            .retain(|(_, node), _| frame.slot_of(*node).is_some());
        self.retain_motion(frame);
        self.settle_at_rest(frame);
    }

    /// css-scroll-snap-1 §6.1: a snapping container must rest on a snap
    /// position. Every commit publishes fresh positions — the first layout,
    /// a relayout that moved the areas, a programmatic scroll the document
    /// applied — so each snapping container no drag is holding, no fling is
    /// driving and no bounce back is moving settles from where it stands,
    /// as an intent like any other scroll.
    fn settle_at_rest(&mut self, frame: &CommittedFrame) {
        for slot in frame.scroll_slots() {
            if self.is_held(slot.node) || self.is_bouncing(slot.node) {
                continue;
            }
            self.settle_node_at_rest(frame, slot);
        }
    }

    /// The at-rest rule for one container: from where it stands to where it
    /// stands, so a `mandatory` axis moves to its nearest position and a
    /// `proximity` one only when within range. A stretched axis is left to
    /// its bounce back.
    fn settle_node_at_rest(&mut self, frame: &CommittedFrame, slot: &dom::ScrollSlot) {
        let (snap_x, snap_y) = frame.snap_axes(slot);
        if snap_x.is_none() && snap_y.is_none() {
            return;
        }
        let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
        let settled = settle_stretch_aware(offset, offset, slot, snap_x, snap_y);
        if settled != offset {
            self.offsets.insert(slot.node, settled);
            self.generation += 1;
        }
    }

    /// Settles every container `pointer`'s drag moved onto a snap position,
    /// from where the drag found it to where it left it, and releases the
    /// hold on them. A stretched axis is not settled: it springs back
    /// first ([`Self::start_bounce_backs`]) and is re-snapped when it lands.
    fn settle(&mut self, frame: &CommittedFrame, pointer: PointerId) {
        self.rebase(frame);
        let held: SmallVec<[(NodeId, Vector2D<f32>); 2]> = self
            .gesture_origins
            .iter()
            .filter(|((held_by, _), _)| *held_by == pointer)
            .map(|((_, node), origin)| (*node, *origin))
            .collect();
        for (node, origin) in held {
            self.gesture_origins.remove(&(pointer, node));
            let Some(index) = frame.slot_of(node) else {
                continue;
            };
            let slot = &frame.scroll_slots()[index as usize];
            let (snap_x, snap_y) = frame.snap_axes(slot);
            let offset = self.offsets.get(&node).copied().unwrap_or(slot.offset);
            let settled = settle_stretch_aware(origin, offset, slot, snap_x, snap_y);
            if settled != offset {
                self.offsets.insert(node, settled);
                self.generation += 1;
            }
        }
    }

    /// Drives one scroll step from the slot `from` names, outward along the
    /// published chain, and reports per axis what was absorbed and where a
    /// container is now stretched. Order and reach are `dom`'s
    /// [`drive_chain`] over the slots' published policy, and each step
    /// lands per [`resolve_elastic_step`] — [`dom::scroll::resolve_step`]'s
    /// rule, the same walk the document runs over live geometry, plus the
    /// `contain-bounce` stretch the document never takes. A drag or fling
    /// step (`pointer` set) records where it found each container it moved,
    /// and a drag step feeds its release velocity; a wheel step snaps as it
    /// lands. Before the walk, a container on the chain already stretched
    /// on an axis unwinds toward its edge first (Lynx's restore-first rule),
    /// whatever the walk's order.
    fn chain(
        &mut self,
        frame: &CommittedFrame,
        from: NodeId,
        delta: Vector2D<f32>,
        motion: Motion,
        pointer: Option<PointerId>,
        at: f64,
    ) -> ChainOutcome {
        self.rebase(frame);
        if motion == Motion::Drag
            && let Some(pointer) = pointer
        {
            self.track_drag(frame, pointer, from, delta, at);
        }
        let slots = frame.scroll_slots();
        let Some(start) = frame.slot_of(from) else {
            return ChainOutcome::default();
        };
        let mut indices: SmallVec<[u32; 4]> = SmallVec::new();
        let mut links: SmallVec<[ChainLink; 4]> = SmallVec::new();
        let mut current = Some(start);
        while let Some(index) = current {
            let slot = &slots[index as usize];
            indices.push(index);
            links.push(slot.link());
            current = slot.parent;
        }
        let mut stretched = ScrollAxes::NONE;
        let mut unwound = Vector2D::zero();
        let mut delta = delta;
        for axis in Axis::BOTH {
            if axis.of(delta) == 0.0 {
                continue;
            }
            for &index in &indices {
                let slot = &slots[index as usize];
                if !axis.flag(slot.bounce) {
                    continue;
                }
                let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
                let (applied, absorbed) = unwind_stretch(
                    motion,
                    axis.of(offset),
                    axis.of(delta),
                    axis.of(slot.max_offset),
                    axis.extent(slot.scrollport),
                );
                if absorbed == 0.0 {
                    continue;
                }
                let mut moved = offset;
                axis.set(&mut moved, applied);
                if let Some(pointer) = pointer {
                    self.gesture_origins
                        .entry((pointer, slot.node))
                        .or_insert(offset);
                }
                self.offsets.insert(slot.node, moved);
                if stretch_of(applied, axis.of(slot.max_offset)) != 0.0 {
                    axis.raise(&mut stretched);
                }
                axis.set(&mut unwound, absorbed);
                let remaining = axis.of(delta) - absorbed;
                axis.set(&mut delta, remaining);
                break;
            }
        }
        let consumed = drive_chain(&links, delta, |link, admitted| {
            let slot = &slots[indices[link] as usize];
            let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
            let (snap_x, snap_y) = frame.snap_axes(slot);
            let step = resolve_elastic_step(
                motion,
                offset,
                admitted,
                slot.max_offset,
                slot.scrollport,
                slot.bounce,
                snap_x,
                snap_y,
            );
            stretched.x |= step.stretched.x;
            stretched.y |= step.stretched.y;
            if step.applied != offset {
                if let Some(pointer) = pointer {
                    self.gesture_origins
                        .entry((pointer, slot.node))
                        .or_insert(offset);
                }
                self.offsets.insert(slot.node, step.applied);
            }
            step.absorbed
        });
        let outcome = ChainOutcome {
            absorbed: unwound + consumed.map_or_else(Vector2D::zero, |(_, absorbed)| absorbed),
            stretched,
        };
        if outcome.consumed() {
            self.generation += 1;
            // A step that landed a container back in range ends the
            // bounce back that was bringing it there.
            self.retain_motion(frame);
        }
        outcome
    }

    pub(super) fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>> {
        self.offsets.get(&node).copied()
    }

    fn refill_due(&self, frame: &CommittedFrame) -> bool {
        self.offsets.iter().any(|(node, offset)| {
            frame.slot_of(*node).is_some_and(|index| {
                let slot = &frame.scroll_slots()[index as usize];
                let (low, high) = slot.encode_window();
                // A stretch is composed from the edge's own content: it
                // asks for no refill of its own.
                let pending = Vector2D::new(
                    clamp_scroll_axis(offset.x, slot.max_offset.x),
                    clamp_scroll_axis(offset.y, slot.max_offset.y),
                );
                axis_refill_due(pending.x, slot.offset.x, low.x, high.x)
                    || axis_refill_due(pending.y, slot.offset.y, low.y, high.y)
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

/// Where an intent may stand on one axis: in range, or — on a
/// `contain-bounce` axis — up to one scrollport past either edge.
fn clamp_intent_axis(value: f32, max: f32, extent: f32, bounces: bool) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    if bounces {
        value.clamp(-extent, max + extent)
    } else {
        value.clamp(0.0, max)
    }
}

/// [`settle_offset`] over an intent that may be stretched: a stretched
/// axis keeps its stretch (its bounce back settles it when it lands), and
/// the rest settle from an in-range `start`.
fn settle_stretch_aware(
    start: Vector2D<f32>,
    offset: Vector2D<f32>,
    slot: &dom::ScrollSlot,
    snap_x: Option<dom::scroll::SnapAxis<'_>>,
    snap_y: Option<dom::scroll::SnapAxis<'_>>,
) -> Vector2D<f32> {
    let start = Vector2D::new(
        clamp_scroll_axis(start.x, slot.max_offset.x),
        clamp_scroll_axis(start.y, slot.max_offset.y),
    );
    let stretched_x = stretch_of(offset.x, slot.max_offset.x) != 0.0;
    let stretched_y = stretch_of(offset.y, slot.max_offset.y) != 0.0;
    let settled = settle_offset(
        start,
        Vector2D::new(
            if stretched_x { start.x } else { offset.x },
            if stretched_y { start.y } else { offset.y },
        ),
        slot.scrollport,
        snap_x,
        snap_y,
    );
    Vector2D::new(
        if stretched_x { offset.x } else { settled.x },
        if stretched_y { offset.y } else { settled.y },
    )
}

/// Composes one frame and renders it into `output`, whatever kind of target
/// that is.
///
/// The one render path: a frame is composed flat, with each scroll slot at
/// the offset the intents carry for it. A frame whose whole program is one
/// unscrolled fragment, asked for at no offset and no animation instant, is
/// rendered straight out of the commit — composing it would copy it.
#[expect(
    clippy::too_many_arguments,
    reason = "one frame's whole path to pixels: the target, the two compose buffers, the frame, \
              and the two pixel tables it draws from"
)]
fn compose_and_render(
    output: &mut Output,
    buffer: &mut Scene,
    filtered: &mut Vec<Option<ImageData>>,
    intents: &ScrollIntents,
    frame: &CommittedFrame,
    images: &[Option<ImageData>],
    size: FrameSize,
    animation_now: Option<f64>,
) -> Result<(), EngineError> {
    // The one optional pre-step. A frame with no `filter: blur()` group and
    // no `backdrop-filter` element — the overwhelming majority — skips it on
    // this one test and touches no offscreen texture at all.
    if frame.filter_groups().is_empty() {
        filtered.clear();
    } else {
        output.prepare_filters(
            frame,
            images,
            &|slot| intents.offset_for(slot.node),
            intents.generation,
            animation_now,
            filtered,
        )?;
    }
    let scene = if intents.offsets.is_empty()
        && animation_now.is_none()
        && let Some(scene) = frame.scene()
    {
        // A filtered frame is never one unscrolled fragment — it carries at
        // least one filter bracket or backdrop op — so this fast path never
        // skips a bake.
        scene
    } else {
        buffer.reset();
        frame.compose_into(
            buffer,
            images,
            filtered,
            &|slot| intents.offset_for(slot.node),
            animation_now,
        );
        &*buffer
    };
    output.render(scene, images, size)
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
        let frame_size = FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let output = Output::build(target, frame_size).await?;
        Ok(Self::with_output(viewport, frame_size, output))
    }

    fn with_output(viewport: Viewport, frame_size: FrameSize, output: Output) -> Self {
        Self {
            seat: Weak::new(),
            frames: None,
            viewport,
            frame_size,
            output,
            occluded: false,
            render_failed: false,
            published: Published::default(),
            redraw_pending: Cell::new(false),
            begin_frames_sent: 0,
            gesture: GestureRouter::default(),
            clock: FrameClock::new(),
            scroll_intents: ScrollIntents::default(),
            composed: None,
            composed_scene: Scene::new(),
            composed_filters: Vec::new(),
            refill_requested_for: None,
            images: images::PainterImages::default(),
            thread_bound: PhantomData,
        }
    }

    /// A painter with nowhere to draw, for the in-crate tests about routing,
    /// events and timers: they pay for no GPU device, and need no executor to
    /// build one.
    #[cfg(test)]
    pub(crate) fn without_output(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        let frame_size = FrameSize::for_viewport(width, height, device_pixel_ratio)
            .expect("an in-crate painter is built at a valid size");
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        Self::with_output(viewport, frame_size, Output::None)
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
        let seat = Rc::new(ViewSeat {
            frame_demand: RefCell::default(),
            commands,
            metrics: watch::channel(Some(viewport)).0,
            images: Rc::new(dom::NoImages),
        });
        let mut painter = Self::without_output(width, height, 1.0);
        painter.seat = Rc::downgrade(&seat);
        painter.frames = Some(frame_receiver);
        (
            painter,
            FarEnd {
                commands: command_receiver,
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
                    crate::link::detached_base(),
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
        if self.render_failed {
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
        // A `BeginFrame` a previous attachment sent and main has not
        // acknowledged yet could otherwise satisfy this attachment's first
        // sequence one frame early. Starting past whatever has been
        // serviced costs at most one extra turn on the next tick.
        self.begin_frames_sent = self
            .begin_frames_sent
            .max(frames.borrow().begin_frame_serviced);
        self.seat = Rc::downgrade(view.seat());
        self.frames = Some(frames);
        self.forget_view();
        self.forget_target();
        self.clock.rebase(view.timeline_epoch());
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
            seat.metrics.send_replace(Some(self.viewport));
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
        self.render_failed
    }

    /// Drops everything derived from a view: what it published, where it was
    /// scrolled to, the gesture in progress, and the pixels resolved out of its
    /// host's store.
    ///
    /// Not what the target holds — that is [`Self::forget_target`], and it
    /// survives a detach, because so do the pixels on screen.
    fn forget_view(&mut self) {
        self.published = Published::default();
        self.refill_requested_for = None;
        self.scroll_intents = ScrollIntents::default();
        self.gesture = GestureRouter::default();
        self.images.forget();
    }

    /// Drops what the draw target holds: the key it was rendered from, and —
    /// for an offscreen target, whose texture is the only copy of that frame —
    /// the frame itself.
    ///
    /// One caller: pointing this painter at a different document. Commit ids
    /// restart at one per document, so a key kept across the change would
    /// answer the new page's first frame with the old page's work — which is
    /// also why `Output::forget` gives up the target's `filter: blur()` bake
    /// cache, keyed the same way.
    fn forget_target(&mut self) {
        self.composed = None;
        self.composed_filters.clear();
        self.output.forget();
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
        if latest.commit() != self.published.commit() {
            if let Some(frame) = latest.frame.as_ref()
                && !self.images.holds(frame.commit_id())
            {
                let Some(seat) = seat.as_ref() else {
                    // The view is gone with its store, so this commit's pixels
                    // can no longer be read. It is not adopted; the painter
                    // keeps the last commit it did read, whole.
                    self.release_seat();
                    return;
                };
                self.images.resolve(frame, seat.images.as_ref());
            }
            self.redraw_pending.set(true);
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

    /// Sends one command to the attached view's task. A detached painter, or
    /// one whose view has ended, says nothing; it goes on showing what it
    /// last drew.
    fn send(&self, command: ToMain) {
        if let Some(seat) = self.seat.upgrade() {
            let _ = seat.commands.send(command);
        }
    }

    /// The newest committed frame this painter has adopted.
    fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.published.frame.as_ref()
    }

    /// Whether the engine owes the timeline another frame, as of the last
    /// pass that polled the view — a `pump`, a `tick`, or an input. That is
    /// when a host asks: after answering the wakeup that carried the frame.
    ///
    /// A detached painter owes nothing: there is no timeline to be behind.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.is_attached()
            && (self.frame().is_some_and(|frame| frame.animations_active())
                || self
                    .seat
                    .upgrade()
                    .is_some_and(|seat| seat.frame_demand.borrow_mut().is_pending())
                || self.gesture.needs_frame()
                || self.scroll_intents.is_animating())
    }

    /// Routes one normalized OS input event against the frame this painter
    /// last read.
    ///
    /// A detached painter routes nothing: it has no frame to hit-test against
    /// and nobody to deliver to, so the event is dropped before the gesture
    /// router sees it rather than opening a sequence that can never close.
    pub fn dispatch_input(&mut self, event: InputEvent) {
        if !self.is_attached() {
            return;
        }
        self.poll_link();
        let at = self.clock.now_seconds();
        let published = self.frame().cloned();
        if let Some(frame) = &published {
            self.scroll_intents.rebase(frame);
        }
        let generation = self.scroll_intents.generation;
        let frame = published.as_deref();
        let animation_now = frame.and_then(|frame| frame.has_exported_curves().then_some(at));
        let target = route_published(frame, &self.scroll_intents, &event, animation_now);
        let mut decisions = InputDecisions::new();
        self.gesture
            .on_input(&event, target, at, &self.published, &mut decisions);
        self.execute_decisions(&mut decisions, published.as_deref(), at);
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
        self.send(ToMain::Refill {
            offsets: self.scroll_intents.writeback(),
        });
    }

    /// Executes one pass's decisions in order.
    ///
    /// `at_seconds` is the clock reading of the pass that produced them — an
    /// input event's arrival, or the tick's own `now` — and is what every
    /// event the pass dispatches reports as its `timestamp`. It is one
    /// reading for the whole pass, so a due `longpress` flushed ahead of the
    /// event that found it and the `tap` synthesized after it are stamped
    /// with that event's arrival, which is the moment they all belong to.
    pub(super) fn execute_decisions(
        &mut self,
        decisions: &mut InputDecisions,
        published: Option<&CommittedFrame>,
        at_seconds: f64,
    ) {
        let mut dispatches = Vec::new();
        {
            let Self {
                gesture,
                scroll_intents,
                ..
            } = &mut *self;
            for decision in decisions.drain(..) {
                match decision {
                    InputDecision::Scroll {
                        pointer,
                        from,
                        delta,
                    } => {
                        let motion = match pointer {
                            Some(_) => Motion::Drag,
                            None => Motion::Wheel,
                        };
                        let consumed = published.is_some_and(|frame| {
                            scroll_intents
                                .chain(frame, from, delta, motion, pointer, at_seconds)
                                .consumed()
                        });
                        if consumed && let Some(pointer) = pointer {
                            gesture.note_scroll_consumed(pointer);
                        }
                    }
                    InputDecision::ScrollEnd { pointer } => {
                        if let Some(frame) = published {
                            scroll_intents.end_drag(frame, pointer, at_seconds);
                        }
                    }
                    InputDecision::Emit(event) => dispatches.push(event),
                }
            }
        }
        let timestamp = at_seconds * 1000.0;
        for event in dispatches {
            if !self.published.listeners.contains(event.name) {
                continue;
            }
            self.send(ToMain::DispatchEvent {
                target: event.target,
                name: event.name,
                payload: InputEventPayload {
                    position: event.position,
                    wheel: event.wheel,
                    touches: event.touches,
                    timestamp,
                },
            });
        }
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
        let published = self.frame().cloned();
        let mut decisions = InputDecisions::new();
        self.gesture.on_tick(now, &self.published, &mut decisions);
        self.execute_decisions(&mut decisions, published.as_deref(), now);
        // The flings and bounce backs advance on the same clock reading,
        // after any decision this tick made about them.
        if let Some(frame) = &published {
            self.scroll_intents.tick(frame, now);
        }
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
        let next_size = FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let moved = self.viewport.width.to_bits() != width.to_bits()
            || self.viewport.height.to_bits() != height.to_bits()
            || self.viewport.device_pixel_ratio.to_bits() != device_pixel_ratio.to_bits();
        if !moved {
            return Ok(());
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        // A detached painter has no seat to write, so it only records the new
        // metrics and imposes them on whichever view it attaches to next.
        self.bind_metrics();
        self.refresh();
        Ok(())
    }

    /// The physical pixel size of this painter's draw target.
    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// Asks for a frame nothing else would have asked for.
    ///
    /// It wakes nobody: every caller is on this thread, inside a call the
    /// host is already making, so the turn that host is in is the turn that
    /// answers it.
    pub fn refresh(&self) {
        self.redraw_pending.set(true);
    }

    fn take_redraw(&self) -> bool {
        self.redraw_pending.replace(false)
    }

    /// Reports whether the window is visible. A window nobody can see draws
    /// nothing, and un-occluding asks again for the frame that was held back.
    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        if !occluded {
            self.refresh();
        }
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
        if self.render_failed {
            return Ok(());
        }
        match self.draw() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.render_failed = true;
                Err(error)
            }
        }
    }

    /// Deliver an actual display vsync reported by the host. Pending requests
    /// participate in `owes_frame`; ordinary pumps and repaints deliver no rAF.
    pub fn vsync(&mut self) {
        self.poll_link();
        if !self.render_failed && !self.occluded {
            self.deliver_vsync(self.clock.now_seconds());
        }
    }

    fn deliver_vsync(&self, now: f64) {
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
        if !main_ticks_due && !always {
            return None;
        }
        self.begin_frames_sent += 1;
        let seq = self.begin_frames_sent;
        seat.commands
            .send(ToMain::BeginFrame { now, seq })
            .ok()
            .map(|()| seq)
    }

    /// Composes `frame` at `key` and renders it into the draw target,
    /// answering whether it rendered anything.
    ///
    /// The one path from a published frame to pixels, whatever the target and
    /// whoever asked: a window's draw, an offscreen tick, and a capture of
    /// either. The caller passes the frame it derived `key` and `animation_now`
    /// from, so what the record names and what the target holds cannot disagree.
    ///
    /// # Errors
    ///
    /// [`EngineError::Render`] or [`EngineError::Gpu`] if the target refuses
    /// the scene, in which case it holds nothing: a failed render is allowed to
    /// have dropped what was there.
    fn render_frame(
        &mut self,
        frame: &CommittedFrame,
        key: ComposeKey,
        animation_now: Option<f64>,
        force: bool,
    ) -> Result<bool, EngineError> {
        let size = self.frame_size;
        if self.composed == Some((key, size)) && !force && animation_now.is_none() {
            return Ok(false);
        }
        let Self {
            output,
            composed_scene,
            composed_filters,
            scroll_intents,
            images,
            ..
        } = &mut *self;
        let rendered = compose_and_render(
            output,
            composed_scene,
            composed_filters,
            scroll_intents,
            frame,
            images.resolved(),
            size,
            animation_now,
        );
        match rendered {
            Ok(()) => {
                self.composed = Some((key, size));
                Ok(true)
            }
            Err(error) => {
                self.composed = None;
                Err(error)
            }
        }
    }

    fn draw(&mut self) -> Result<(), EngineError> {
        if !matches!(self.output, Output::Window(_)) || self.occluded {
            return Ok(());
        }
        if !self.take_redraw() && !self.is_animating() {
            return Ok(());
        }
        // Nothing has been committed yet, so there is nothing to put on the
        // window. Acquiring an image for it would wait on vsync natively and,
        // in a browser, present an untouched one — blanking whatever the
        // previous page left on the canvas. The redraw this consumed comes
        // back when the first commit arrives, which is a change of commit and
        // so asks for one itself.
        let Some(frame) = self.frame().cloned() else {
            return Ok(());
        };
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
                    self.refresh();
                    return Ok(());
                }
            }
        };
        let now = self.clock.now_seconds();
        self.tick_gestures(now);
        let _ = self.begin_frame(now, false);
        self.scroll_intents.rebase(&frame);
        self.maybe_request_refill(&frame);
        let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
        let animation_now = frame.has_live_curves().then_some(now);
        self.render_frame(&frame, key, animation_now, false)?;
        // Presented whether or not this turn rendered: a surface that was
        // invalidated or re-exposed puts the retained frame up again with a
        // blit alone.
        let Output::Window(graphics) = &mut self.output else {
            unreachable!("the window output was just checked");
        };
        graphics.present(acquired);
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
        // Refused before anything is composed, so a painter with no target says
        // so rather than reporting the empty target a compose would leave.
        #[cfg(test)]
        if matches!(self.output, Output::None) {
            return Err(EngineError::Render(
                "this painter has no draw target to capture".to_owned(),
            ));
        }
        let size = self.frame_size;
        let now = self.clock.now_seconds();
        self.poll_link();
        // The newest frame is brought to the target first, so a capture is of
        // what this painter would draw rather than of whatever it last drew.
        if let Some(frame) = self.frame().cloned() {
            let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
            let animation_now = frame.has_live_curves().then_some(now);
            self.render_frame(&frame, key, animation_now, false)?;
        }
        // Read back only what the record names at the size being read: a
        // record at another size is not this capture's pixels.
        if !self.composed.is_some_and(|(_, rendered)| rendered == size) {
            return Err(EngineError::Render(
                "no frame has been rendered to capture".to_owned(),
            ));
        }
        let pixels = match &mut self.output {
            #[cfg(test)]
            Output::None => unreachable!("a painter with no target was refused above"),
            Output::Offscreen(gpu) => gpu
                .read_pixels()
                .map_err(|error| EngineError::Gpu(error.to_string()))?,
            Output::Window(graphics) => graphics.capture_frame(size)?,
        };
        Ok(Screenshot { size, pixels })
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
    /// [`Self::tick`].
    #[must_use]
    pub fn owes_frame(&self) -> bool {
        if self.render_failed
            || self.occluded
            || !self.is_attached()
            || !matches!(self.output, Output::Window(_))
        {
            return false;
        }
        self.is_animating() || self.redraw_pending.get()
    }

    /// The newest committed frame, for the seams outside this module that
    /// read one.
    #[cfg(test)]
    pub(crate) fn published_frame(&mut self) -> Option<Arc<CommittedFrame>> {
        self.poll_link();
        self.frame().cloned()
    }

    /// Advances an offscreen painter by one frame, answering whether it drew.
    /// Supplies one synthetic vsync to outstanding script-frame requests first.
    ///
    /// The one call that blocks this thread on `bobcat-main`, which is why
    /// only an offscreen painter has it — and why a browser painter, which
    /// cannot have an offscreen target at all, can never reach it. It asks
    /// the view's host for nothing either: the view's own `pump` is what
    /// services its resource protocol, so a host settling an image drives
    /// both turns.
    ///
    /// # Errors
    ///
    /// [`EngineError::NotOffscreen`] if this painter presents into a window —
    /// its frames come from [`Self::pump`], on the host's own clock — and
    /// [`EngineError::Gpu`] if the render fails.
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        if !matches!(self.output, Output::Offscreen(_)) {
            return Err(EngineError::NotOffscreen);
        }
        let now = self.clock.now_seconds();
        self.service_gesture_clock(now);
        self.deliver_vsync(now);
        if let Some(seq) = self.begin_frame(now, true) {
            let _ = self.wait_begin_frame(seq, BEGIN_FRAME_TIMEOUT);
        }
        self.poll_link();
        let Some(frame) = self.frame().cloned() else {
            return Ok(false);
        };
        self.scroll_intents.rebase(&frame);
        self.maybe_request_refill(&frame);
        let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
        let animation_now = frame.has_live_curves().then_some(now);
        let rendered = self.render_frame(&frame, key, animation_now, force)?;
        if rendered {
            // The submitted work is waited for here rather than in
            // `Output::render`: a capture's readback polls the device itself,
            // and this is the caller that asks for pixels nothing reads back.
            let Output::Offscreen(gpu) = &self.output else {
                unreachable!("the offscreen output was just checked");
            };
            gpu.wait_idle()
                .map_err(|error| EngineError::Gpu(error.to_string()))?;
        }
        Ok(rendered)
    }

    /// Waits for the attached view to acknowledge a particular `BeginFrame`,
    /// adopting everything it publishes on the way.
    ///
    /// The one blocking wait a host's own thread makes on `bobcat-main`, and
    /// [`Self::tick`] — offscreen only — is the one call that reaches it. It
    /// ends early on a view whose tasks have gone, because nothing will
    /// service that sequence number then; a boot that failed ends those
    /// tasks, so a failure ends this wait rather than letting it run out its
    /// whole deadline.
    pub(super) fn wait_begin_frame(&mut self, seq: u64, timeout: Duration) -> bool {
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
            match block_on_deadline(frames.changed(), deadline) {
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
