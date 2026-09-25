//! What a scroll does after the finger: the fling a release velocity
//! carries down the chain, the stretch a `contain-bounce` boundary takes,
//! and the spring back — [`super::motion`]'s curves, run over the scroll
//! intents on the painter's own clock.
//!
//! Everything here is a motion of the intents alone: a fling step is one
//! more chain walk over the published slot table, a bounce back moves one
//! offset per frame, and neither recommits, waits on the main thread, or
//! involves the input router or any event — the same contract a drag step
//! has. Each step's offsets are posted to the main thread like a drag
//! step's, and the step that ends a fling or lands a bounce back posts the
//! container's rest. The curves and their constants are `motion`'s; what this module
//! adds is when each starts and stops:
//!
//! - A drag's release velocity is measured here, from the drag's own steps over its last
//!   [`VELOCITY_WINDOW_SECONDS`] — the intents see every step the drag decided, stamped with the
//!   clock reading it arrived at. A release with velocity starts a [`Fling`] from the slot the drag
//!   latched, aimed at the snap position its whole travel would settle on when that slot snaps.
//!   Each frame the fling's distance since the last is chained as a [`Motion::Fling`] step. An axis
//!   the chain cannot move at all is spent; an axis that stretched past a boundary switches to the
//!   overshoot rate. In range a fling is over when what is left of its curve is under one physical
//!   pixel; stretched, when its velocity is (lynx-ui's rule, so the bounce back starts promptly).
//! - A stretched container nothing is holding or flinging starts a [`BounceBack`] per stretched
//!   axis: a drag's release without velocity, a fling's end.
//! - A drag's first step on a chain ([`ScrollIntents::interrupt`]) stops the fling and the bounce
//!   backs on it: the containers stay where the finger found them, held like any the drag moves,
//!   and the release decides again.
//!
//! The drag's own holds ([`ScrollIntents::gesture_origins`]) outlive the
//! drag for as long as its fling runs, so a commit landing mid-fling does
//! not re-snap a container out from under the curve.

use dom::input::PointerId;
use dom::scroll::{ScrollAxes, SnapAxis};
use dom::{CommittedFrame, NodeId, Size2D, Vector2D};
use smallvec::SmallVec;

use super::motion::{
    FLING_DECAY_PER_MS, Motion, OVERSHOOT_DECAY_PER_MS, bounce_back, fling_distance, fling_travel,
    fling_velocity, fling_velocity_for_travel, rest_threshold, rubber_band_slope,
    rubber_band_travel, stretch_of,
};
use super::{ScrollIntents, note_changed};

/// How far back a release looks for its velocity: the drag's travel over
/// the steps inside this window, divided by their span. A drag whose last
/// step is older than this lifted from rest and flings nothing. Android's
/// `VelocityTracker` horizon.
pub(crate) const VELOCITY_WINDOW_SECONDS: f64 = 0.1;

/// The least span of steps a release velocity is measured over. Under it —
/// the drag's steps all in one host tick — there is no motion to measure,
/// only a division by nothing.
pub(crate) const VELOCITY_MIN_SPAN_SECONDS: f64 = 0.005;

/// The most recent steps a drag keeps for its release velocity.
const VELOCITY_SAMPLES: usize = 8;

/// One pointer's drag as the intents see it: the slot it chains from and
/// its recent travel, for the release velocity.
#[derive(Debug, Clone)]
pub(super) struct DragTrack {
    /// The slot the drag latched, which its steps chain from.
    from: NodeId,
    /// The drag's whole scroll delta so far.
    travelled: Vector2D<f32>,
    /// `(clock seconds, travelled)` after each step, oldest first.
    samples: SmallVec<[(f64, Vector2D<f32>); VELOCITY_SAMPLES]>,
}

impl DragTrack {
    fn new(from: NodeId) -> Self {
        Self {
            from,
            travelled: Vector2D::zero(),
            samples: SmallVec::new(),
        }
    }

    fn step(&mut self, at: f64, delta: Vector2D<f32>) {
        self.travelled += delta;
        if self.samples.len() == VELOCITY_SAMPLES {
            self.samples.remove(0);
        }
        self.samples.push((at, self.travelled));
    }

    /// The drag's velocity at `at`, in CSS px per millisecond of scroll
    /// delta, over the steps inside [`VELOCITY_WINDOW_SECONDS`]; zero when
    /// the newest step is itself outside the window (the finger had
    /// stopped) or the steps span less than [`VELOCITY_MIN_SPAN_SECONDS`].
    fn release_velocity(&self, at: f64) -> Vector2D<f32> {
        let Some(&(newest_at, newest)) = self.samples.last() else {
            return Vector2D::zero();
        };
        if at - newest_at > VELOCITY_WINDOW_SECONDS {
            return Vector2D::zero();
        }
        let Some(&(oldest_at, oldest)) = self
            .samples
            .iter()
            .find(|(sampled_at, _)| newest_at - sampled_at <= VELOCITY_WINDOW_SECONDS)
        else {
            return Vector2D::zero();
        };
        if newest_at - oldest_at < VELOCITY_MIN_SPAN_SECONDS {
            return Vector2D::zero();
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a span of milliseconds is well inside f32"
        )]
        let span_ms = ((newest_at - oldest_at) * 1000.0) as f32;
        (newest - oldest) / span_ms
    }
}

/// One fling: a drag's release velocity still travelling down its chain.
#[derive(Debug, Clone)]
pub(super) struct Fling {
    /// The slot the drag latched: every step chains from here, as the
    /// drag's steps did.
    from: NodeId,
    /// The drag's pointer, whose holds this fling keeps until it ends.
    pointer: PointerId,
    /// CSS px per millisecond, in scroll direction.
    velocity: Vector2D<f32>,
    /// The axes on which the fling has stretched past a boundary, and so
    /// decays at the overshoot rate.
    stretched: ScrollAxes,
    /// The clock reading of the last step.
    last: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    X,
    Y,
}

impl Axis {
    pub(super) const BOTH: [Self; 2] = [Self::X, Self::Y];

    pub(super) fn of(self, vector: Vector2D<f32>) -> f32 {
        match self {
            Self::X => vector.x,
            Self::Y => vector.y,
        }
    }

    pub(super) fn set(self, vector: &mut Vector2D<f32>, value: f32) {
        match self {
            Self::X => vector.x = value,
            Self::Y => vector.y = value,
        }
    }

    pub(super) fn flag(self, axes: ScrollAxes) -> bool {
        match self {
            Self::X => axes.x,
            Self::Y => axes.y,
        }
    }

    pub(super) fn raise(self, axes: &mut ScrollAxes) {
        match self {
            Self::X => axes.x = true,
            Self::Y => axes.y = true,
        }
    }

    pub(super) fn extent(self, size: Size2D<f32>) -> f32 {
        match self {
            Self::X => size.width,
            Self::Y => size.height,
        }
    }
}

/// Which boundary a stretch is past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Start,
    End,
}

/// One container's axis springing back from a stretch.
#[derive(Debug, Clone)]
pub(super) struct BounceBack {
    node: NodeId,
    axis: Axis,
    /// Read against the slot's `max_offset` at each step, so a relayout
    /// mid-bounce lands on the edge the new geometry has.
    edge: Edge,
    /// The stretch it started from, signed like [`stretch_of`].
    displacement: f32,
    started: f64,
}

/// What one chain walk did, per axis.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ChainOutcome {
    pub(super) absorbed: Vector2D<f32>,
    pub(super) stretched: ScrollAxes,
}

impl ChainOutcome {
    pub(super) fn consumed(&self) -> bool {
        self.absorbed != Vector2D::zero()
    }
}

/// The scroll containers on the chain from `from`, nearest first.
pub(super) fn chain_nodes(frame: &CommittedFrame, from: NodeId) -> SmallVec<[NodeId; 4]> {
    let slots = frame.scroll_slots();
    let mut nodes = SmallVec::new();
    let mut current = frame.slot_of(from);
    while let Some(index) = current {
        let slot = &slots[index as usize];
        nodes.push(slot.node);
        current = slot.parent;
    }
    nodes
}

impl ScrollIntents {
    /// Whether a fling or a bounce back is in progress, and so owes the
    /// timeline another frame.
    pub(super) fn is_animating(&self) -> bool {
        !self.flings.is_empty() || !self.bounce_backs.is_empty()
    }

    fn offset_of_slot(&self, frame: &CommittedFrame, node: NodeId) -> Option<Vector2D<f32>> {
        let index = frame.slot_of(node)?;
        Some(
            self.offsets
                .get(&node)
                .copied()
                .unwrap_or(frame.scroll_slots()[index as usize].offset),
        )
    }

    fn set_offset(&mut self, node: NodeId, offset: Vector2D<f32>) {
        if self.offsets.get(&node).copied() != Some(offset) {
            self.write(node, offset);
            self.generation += 1;
        }
    }

    /// Records one drag step for `pointer`'s release velocity, and — on
    /// its first step from `from` — stops the fling and the bounce backs
    /// on that chain, leaving their containers where the finger found
    /// them, held by `pointer`.
    pub(super) fn track_drag(
        &mut self,
        frame: &CommittedFrame,
        pointer: PointerId,
        from: NodeId,
        delta: Vector2D<f32>,
        at: f64,
    ) {
        let fresh = self
            .drags
            .get(&pointer)
            .is_none_or(|track| track.from != from);
        if fresh {
            self.drags.insert(pointer, DragTrack::new(from));
            self.interrupt(frame, from, pointer);
        }
        if let Some(track) = self.drags.get_mut(&pointer) {
            track.step(at, delta);
        }
    }

    /// Stops the fling and the bounce backs on the chain from `from`; their
    /// containers become `pointer`'s holds, from where they stand.
    fn interrupt(&mut self, frame: &CommittedFrame, from: NodeId, pointer: PointerId) {
        let chain = chain_nodes(frame, from);
        if chain.is_empty() {
            return;
        }
        for fling in std::mem::take(&mut self.flings) {
            let shared =
                chain.contains(&fling.from) || chain_nodes(frame, fling.from).contains(&from);
            if !shared {
                self.flings.push(fling);
                continue;
            }
            let held: SmallVec<[NodeId; 2]> = self
                .gesture_origins
                .keys()
                .filter(|(held_by, _)| *held_by == fling.pointer)
                .map(|(_, node)| *node)
                .collect();
            for node in held {
                self.gesture_origins.remove(&(fling.pointer, node));
                if let Some(offset) = self.offset_of_slot(frame, node) {
                    self.gesture_origins.insert((pointer, node), offset);
                }
            }
        }
        for back in std::mem::take(&mut self.bounce_backs) {
            if !chain.contains(&back.node) {
                self.bounce_backs.push(back);
                continue;
            }
            if let Some(offset) = self.offset_of_slot(frame, back.node) {
                self.gesture_origins
                    .entry((pointer, back.node))
                    .or_insert(offset);
            }
        }
    }

    /// A scrolling drag's release at `now`: flings its chain at the
    /// velocity its last steps measured, or — with none worth a curve —
    /// settles what it held and lets any stretch spring back.
    pub(super) fn end_drag(&mut self, frame: &CommittedFrame, pointer: PointerId, now: f64) {
        self.rebase(frame);
        let track = self.drags.remove(&pointer);
        let threshold = rest_threshold(frame.device_pixel_ratio());
        let Some((from, start)) = track
            .as_ref()
            .and_then(|track| Some((track.from, frame.slot_of(track.from)?)))
        else {
            self.settle(frame, pointer);
            self.start_bounce_backs(frame, now);
            return;
        };
        let velocity = track
            .as_ref()
            .map_or_else(Vector2D::zero, |track| track.release_velocity(now));
        let slot = &frame.scroll_slots()[start as usize];
        let offset = self.offset_of_slot(frame, slot.node).unwrap_or(slot.offset);
        let origin = self
            .gesture_origins
            .get(&(pointer, slot.node))
            .copied()
            .unwrap_or(offset);
        let (snap_x, snap_y) = frame.snap_axes(slot);
        let mut velocity = velocity;
        let mut stretched = ScrollAxes::NONE;
        for axis in Axis::BOTH {
            let mut v = axis.of(velocity);
            if !v.is_finite() {
                v = 0.0;
            }
            // Let go mid-stretch: the finger's velocity is damped to the
            // stretch's by the rubber band's slope there, and the fling
            // carries on at the overshoot rate.
            if let Some((stretch, extent)) = self.stretched_hold(frame, pointer, axis) {
                v *= rubber_band_slope(rubber_band_travel(stretch.abs(), extent), extent);
                axis.raise(&mut stretched);
            } else if v != 0.0 {
                // Aimed at the snap position the whole curve would settle
                // on, so the fling ends there rather than snapping after.
                let snap = match axis {
                    Axis::X => snap_x,
                    Axis::Y => snap_y,
                };
                if let Some(snap) = snap {
                    let current = axis.of(offset);
                    let max = axis.of(slot.max_offset);
                    let predicted = (current + fling_travel(v, FLING_DECAY_PER_MS)).clamp(0.0, max);
                    let target = snap.settle(
                        axis.of(origin),
                        predicted,
                        SnapAxis::proximity_threshold(axis.extent(slot.scrollport)),
                    );
                    v = fling_velocity_for_travel(target - current, FLING_DECAY_PER_MS);
                }
            }
            axis.set(&mut velocity, v);
        }
        let mut fling = Fling {
            from,
            pointer,
            velocity,
            stretched,
            last: now,
        };
        fling.rest(threshold);
        if fling.velocity == Vector2D::zero() {
            self.settle(frame, pointer);
            self.start_bounce_backs(frame, now);
            return;
        }
        self.flings.push(fling);
    }

    /// A container `pointer` holds that is stretched on `axis`: its stretch
    /// and the scrollport extent it stretched against.
    fn stretched_hold(
        &self,
        frame: &CommittedFrame,
        pointer: PointerId,
        axis: Axis,
    ) -> Option<(f32, f32)> {
        self.gesture_origins
            .keys()
            .filter(|(held_by, _)| *held_by == pointer)
            .find_map(|(_, node)| {
                let index = frame.slot_of(*node)?;
                let slot = &frame.scroll_slots()[index as usize];
                if !axis.flag(slot.bounce) {
                    return None;
                }
                let offset = self.offset_of_slot(frame, *node)?;
                let stretch = stretch_of(axis.of(offset), axis.of(slot.max_offset));
                (stretch != 0.0).then_some((stretch, axis.extent(slot.scrollport)))
            })
    }

    /// Advances every fling and bounce back to `now`.
    pub(super) fn tick(&mut self, frame: &CommittedFrame, now: f64) {
        if !self.is_animating() {
            return;
        }
        self.rebase(frame);
        self.tick_flings(frame, now);
        self.tick_bounce_backs(frame, now);
    }

    fn tick_flings(&mut self, frame: &CommittedFrame, now: f64) {
        let threshold = rest_threshold(frame.device_pixel_ratio());
        for mut fling in std::mem::take(&mut self.flings) {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "a frame's worth of milliseconds is well inside f32"
            )]
            let elapsed_ms = ((now - fling.last).max(0.0) * 1000.0) as f32;
            fling.last = now;
            let mut delta = Vector2D::zero();
            for axis in Axis::BOTH {
                let v = axis.of(fling.velocity);
                let rate = fling.rate(axis);
                axis.set(&mut delta, fling_distance(v, rate, elapsed_ms));
                axis.set(&mut fling.velocity, fling_velocity(v, rate, elapsed_ms));
            }
            if delta != Vector2D::zero() {
                let outcome = self.chain(
                    frame,
                    fling.from,
                    delta,
                    Motion::Fling,
                    Some(fling.pointer),
                    now,
                );
                for axis in Axis::BOTH {
                    // A wall — every reachable container at its boundary,
                    // none of them stretching — spends the axis.
                    if axis.of(delta) != 0.0 && axis.of(outcome.absorbed) == 0.0 {
                        axis.set(&mut fling.velocity, 0.0);
                    }
                    if axis.flag(outcome.stretched) {
                        axis.raise(&mut fling.stretched);
                    }
                }
            }
            fling.rest(threshold);
            if fling.velocity == Vector2D::zero() {
                self.settle(frame, fling.pointer);
                self.start_bounce_backs(frame, now);
            } else {
                self.flings.push(fling);
            }
        }
    }

    fn tick_bounce_backs(&mut self, frame: &CommittedFrame, now: f64) {
        let threshold = rest_threshold(frame.device_pixel_ratio());
        for back in std::mem::take(&mut self.bounce_backs) {
            let Some(index) = frame.slot_of(back.node) else {
                continue;
            };
            let slot = &frame.scroll_slots()[index as usize];
            let Some(mut offset) = self.offset_of_slot(frame, back.node) else {
                continue;
            };
            let edge = match back.edge {
                Edge::Start => 0.0,
                Edge::End => back.axis.of(slot.max_offset),
            };
            #[allow(
                clippy::cast_possible_truncation,
                reason = "a bounce's seconds are well inside f32"
            )]
            let elapsed = (now - back.started).max(0.0) as f32;
            let stretch = bounce_back(back.displacement, elapsed);
            let done = stretch.abs() < threshold;
            back.axis
                .set(&mut offset, if done { edge } else { edge + stretch });
            self.set_offset(back.node, offset);
            if done {
                // Back on its edge, which a snapping container may not
                // rest on: the at-rest rule applies as after any commit.
                self.settle_node_at_rest(frame, slot);
            } else {
                self.bounce_backs.push(back);
            }
        }
    }

    /// Starts a bounce back for every stretched axis of every container
    /// nothing is holding or flinging.
    pub(super) fn start_bounce_backs(&mut self, frame: &CommittedFrame, now: f64) {
        for slot in frame.scroll_slots() {
            if slot.bounce == ScrollAxes::NONE {
                continue;
            }
            // Only an intent can be stretched: a committed offset never is.
            let Some(offset) = self.offsets.get(&slot.node).copied() else {
                continue;
            };
            if self.is_held(slot.node) || self.is_flinging(frame, slot.node) {
                continue;
            }
            for axis in Axis::BOTH {
                if !axis.flag(slot.bounce) {
                    continue;
                }
                let stretch = stretch_of(axis.of(offset), axis.of(slot.max_offset));
                if stretch == 0.0
                    || self
                        .bounce_backs
                        .iter()
                        .any(|back| back.node == slot.node && back.axis == axis)
                {
                    continue;
                }
                self.bounce_backs.push(BounceBack {
                    node: slot.node,
                    axis,
                    edge: if stretch < 0.0 {
                        Edge::Start
                    } else {
                        Edge::End
                    },
                    displacement: stretch,
                    started: now,
                });
            }
        }
    }

    /// Whether some drag holds `node`.
    pub(super) fn is_held(&self, node: NodeId) -> bool {
        self.gesture_origins.keys().any(|(_, held)| *held == node)
    }

    /// Whether some fling's chain runs through `node`.
    pub(super) fn is_flinging(&self, frame: &CommittedFrame, node: NodeId) -> bool {
        self.flings
            .iter()
            .any(|fling| chain_nodes(frame, fling.from).contains(&node))
    }

    /// Whether a bounce back is moving `node`.
    pub(super) fn is_bouncing(&self, node: NodeId) -> bool {
        self.bounce_backs.iter().any(|back| back.node == node)
    }

    /// Drops what no longer applies to `frame`: a fling whose latched slot
    /// is gone, a bounce back whose container is gone or no longer
    /// stretched, a drag whose slot is gone. A dropped bounce back records
    /// its container: a commit that widened the range ends one without a
    /// step, and the offset main holds clamped to the old edge is posted
    /// again, at rest.
    pub(super) fn retain_motion(&mut self, frame: &CommittedFrame) {
        self.flings
            .retain(|fling| frame.slot_of(fling.from).is_some());
        self.drags
            .retain(|_, track| frame.slot_of(track.from).is_some());
        let offsets = &self.offsets;
        let changed = &mut self.changed;
        self.bounce_backs.retain(|back| {
            let stretched = frame.slot_of(back.node).is_some_and(|index| {
                let slot = &frame.scroll_slots()[index as usize];
                offsets.get(&back.node).is_some_and(|offset| {
                    stretch_of(back.axis.of(*offset), back.axis.of(slot.max_offset)) != 0.0
                })
            });
            if !stretched {
                note_changed(changed, back.node);
            }
            stretched
        });
    }
}

impl Fling {
    fn rate(&self, axis: Axis) -> f32 {
        if axis.flag(self.stretched) {
            OVERSHOOT_DECAY_PER_MS
        } else {
            FLING_DECAY_PER_MS
        }
    }

    /// Spends each axis that has come to rest: in range, one whose
    /// remaining curve is under `threshold`; stretched, one whose velocity
    /// is (so the bounce back starts promptly — lynx-ui's rule).
    fn rest(&mut self, threshold: f32) {
        for axis in Axis::BOTH {
            let v = axis.of(self.velocity);
            let spent = if axis.flag(self.stretched) {
                v.abs() <= threshold
            } else {
                fling_travel(v.abs(), FLING_DECAY_PER_MS) < threshold
            };
            if spent {
                axis.set(&mut self.velocity, 0.0);
            }
        }
    }
}
