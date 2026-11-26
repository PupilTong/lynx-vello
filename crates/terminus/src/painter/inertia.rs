//! What a scroll does after the finger: the fling a release velocity
//! carries down the chain, the stretch a `contain-bounce` boundary takes,
//! and the spring back — [`super::motion`]'s curves, run over the scroll
//! intents on the painter's own clock.
//!
//! Everything here is a motion of the intents alone: a fling step is one
//! more chain walk over the published slot table, a bounce back moves one
//! offset per frame, and neither recommits, waits on the document's thread,
//! or involves the input router or any event — the same contract a drag step
//! has. Each step's offsets are posted to the document like a drag step's, and the step that ends a
//! fling or lands a bounce back posts the container's rest. The curves and their constants are
//! `motion`'s; what this module adds is when each starts and stops:
//!
//! - A drag's release velocity is measured here, from the drag's own steps over its last
//!   [`VELOCITY_WINDOW_SECONDS`] — the intents see every step the drag decided, stamped with the
//!   clock reading it arrived at. A release with velocity starts a [`Fling`] from the slot the drag
//!   latched, aimed at the snap position its whole travel would settle on when that slot snaps.
//!   Each frame the fling's distance since the last is chained as a [`Motion::Fling`] step. An axis
//!   the chain cannot move at all is spent; an axis that stretched past a boundary switches to the
//!   overshoot rate. In range a fling is over when what is left of its curve is under one physical
//!   pixel; stretched, when its velocity is, so the bounce back starts promptly.
//! - A stretched container nothing is holding or flinging starts a [`BounceBack`] per stretched
//!   axis: a drag's release without velocity, a fling's end.
//! - A container that has to move to a snap position once the finger is off starts a [`Glide`] per
//!   axis instead of jumping there. On the axis the drag's latched slot snaps on, the release picks
//!   the snap position its whole fling would settle on, as before; a release without velocity
//!   glides there from rest, and one with velocity glides there starting at that velocity (limited
//!   so the spring cannot pass the position) when the position is within one scrollport, where a
//!   fling aimed at it would spend most of its time on the curve's last pixels. A farther position
//!   is still reached by the aimed fling. Every other container the drag or fling let go settles by
//!   gliding ([`ScrollIntents::settle`]), and a smooth programmatic scroll is a glide from rest
//!   (`ScrollIntents::apply_request`). A distance under one physical pixel is written at once.
//! - A drag's step on a chain ([`ScrollIntents::interrupt`]) stops what moves on it — the fling and
//!   the bounce backs on its first step, a glide on any step — and the containers stay where the
//!   finger found them, held like any the drag moves, and the release decides again.
//! - A wheel step that moves a gliding container ends its glides ([`ScrollIntents::stop_glides`]):
//!   the wheel's own landing position, already snapped, stands.
//! - An `overscroll-behavior: circular` axis has no wall and no stretch: every fling step on it is
//!   absorbed, so the fling there ends by its decay alone, and the release aims at a snap position
//!   whatever copy of it the whole travel reaches across the seam.
//! - An [`AutoScroll`] is the one motion a script starts and ends: an axis moving at a constant
//!   rate ([`crate::scroll::AutoScrollRequest`]). It is the last to move a container: while a drag
//!   holds it, or a fling, a bounce back or a glide moves it, the auto-scroll adds nothing, and it
//!   goes on from where they left the container. Nothing but another change of the container's
//!   auto-scroll ends it, except the edge it moves toward when it was asked to stop there. It moves
//!   its own container only, clamps at the edge and never stretches, and an axis that wraps has no
//!   edge to end it. A write is at least one physical pixel and at least
//!   [`AUTO_SCROLL_MIN_INTERVAL_SECONDS`] after the last, so a slow rate or a fast display does not
//!   recompose a frame per refresh; a container whose auto-scroll cannot move — held, or at its
//!   edge — owes no frame on its account ([`ScrollIntents::is_auto_scrolling`]).
//!
//! The drag's own holds ([`ScrollIntents::gesture_origins`]) outlive the
//! drag for as long as its fling runs, so a commit landing mid-fling does
//! not re-snap a container out from under the curve; a gliding container
//! is exempt from that at-rest rule on its own.

use euclid::default::{Size2D, Vector2D};
use smallvec::SmallVec;

use super::motion::{
    FLING_DECAY_PER_MS, Motion, OVERSHOOT_DECAY_PER_MS, bounce_back, fling_distance, fling_travel,
    fling_velocity, fling_velocity_for_travel, glide, glide_velocity, rest_threshold,
    rubber_band_slope, rubber_band_travel, stretch_of,
};
use super::{ScrollIntents, note_changed, wrap_axes};
use crate::input::PointerId;
use crate::scroll::{AutoScrollRequest, ScrollAxes, SnapAxis};
use crate::tree::document::NodeId;
use crate::visual::CommittedFrame;

pub(super) const VELOCITY_WINDOW_SECONDS: f64 = 0.1;

pub(super) const WHEEL_REST_SECONDS: f64 = 0.1;

pub(super) const VELOCITY_MIN_SPAN_SECONDS: f64 = 0.005;

const VELOCITY_SAMPLES: usize = 8;

pub(super) const AUTO_SCROLL_MIN_INTERVAL_SECONDS: f64 = 1.0 / 80.0;

#[derive(Debug, Clone)]
pub(super) struct DragTrack {
    from: NodeId,
    travelled: Vector2D<f32>,
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

#[derive(Debug, Clone)]
pub(super) struct Fling {
    from: NodeId,
    pointer: PointerId,
    velocity: Vector2D<f32>,
    stretched: ScrollAxes,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Start,
    End,
}

#[derive(Debug, Clone)]
pub(super) struct BounceBack {
    node: NodeId,
    axis: Axis,
    edge: Edge,
    displacement: f32,
    started: f64,
}

#[derive(Debug, Clone)]
pub(super) struct Glide {
    node: NodeId,
    axis: Axis,
    target: f32,
    displacement: f32,
    velocity: f32,
    started: f64,
}

impl Glide {
    pub(super) fn new(
        node: NodeId,
        axis: Axis,
        from: f32,
        target: f32,
        velocity: f32,
        now: f64,
    ) -> Self {
        let displacement = from - target;
        Self {
            node,
            axis,
            target,
            displacement,
            velocity: glide_velocity(displacement, velocity),
            started: now,
        }
    }

    fn remaining(&self, now: f64) -> f32 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a glide's seconds are well inside f32"
        )]
        let elapsed = (now - self.started).max(0.0) as f32;
        glide(self.displacement, self.velocity, elapsed)
    }
}

#[derive(Debug, Clone)]
pub(super) struct AutoScroll {
    node: NodeId,
    axis: Axis,
    rate: f32,
    stop_at_edge: bool,
    since: Option<f64>,
}

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
    #[must_use]
    pub(super) fn is_animating(&self) -> bool {
        !self.flings.is_empty()
            || !self.bounce_backs.is_empty()
            || !self.glides.is_empty()
            || !self.wheel_steps.is_empty()
    }

    pub(super) fn note_wheel_step(&mut self, node: NodeId, at: f64) {
        match self.wheel_steps.iter_mut().find(|(held, _)| *held == node) {
            Some((_, last)) => *last = at,
            None => self.wheel_steps.push((node, at)),
        }
    }

    pub(super) fn end_wheel_steps(&mut self, node: NodeId) {
        if !self.wheel_steps.is_empty() {
            self.wheel_steps.retain(|(held, _)| *held != node);
        }
    }

    #[must_use]
    pub(super) fn is_wheeling(&self, node: NodeId) -> bool {
        self.wheel_steps.iter().any(|(held, _)| *held == node)
    }

    fn tick_wheel_steps(&mut self, now: f64) {
        let Self {
            wheel_steps,
            changed,
            ..
        } = self;
        wheel_steps.retain(|(node, last)| {
            let quiet = now - *last >= WHEEL_REST_SECONDS;
            if quiet {
                note_changed(changed, *node);
            }
            !quiet
        });
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
        } else if !self.glides.is_empty() {
            self.interrupt_glides(frame, &chain_nodes(frame, from), pointer);
        }
        if let Some(track) = self.drags.get_mut(&pointer) {
            track.step(at, delta);
        }
    }

    fn interrupt(&mut self, frame: &CommittedFrame, from: NodeId, pointer: PointerId) {
        let chain = chain_nodes(frame, from);
        if chain.is_empty() {
            return;
        }
        self.interrupt_glides(frame, &chain, pointer);
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

    fn interrupt_glides(&mut self, frame: &CommittedFrame, chain: &[NodeId], pointer: PointerId) {
        for glide in std::mem::take(&mut self.glides) {
            if !chain.contains(&glide.node) {
                self.glides.push(glide);
                continue;
            }
            if let Some(offset) = self.offset_of_slot(frame, glide.node) {
                self.gesture_origins
                    .entry((pointer, glide.node))
                    .or_insert(offset);
            }
        }
    }

    pub(super) fn end_drag(&mut self, frame: &CommittedFrame, pointer: PointerId, now: f64) {
        self.rebase(frame, now);
        let track = self.drags.remove(&pointer);
        let threshold = rest_threshold(frame.device_pixel_ratio());
        let Some((from, start)) = track
            .as_ref()
            .and_then(|track| Some((track.from, frame.slot_of(track.from)?)))
        else {
            self.settle(frame, pointer, now);
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
            if let Some((stretch, extent)) = self.stretched_hold(frame, pointer, axis) {
                v *= rubber_band_slope(rubber_band_travel(stretch.abs(), extent), extent);
                axis.raise(&mut stretched);
            } else if let Some(snap) = match axis {
                Axis::X => snap_x,
                Axis::Y => snap_y,
            } {
                let current = axis.of(offset);
                let max = axis.of(slot.max_offset);
                let predicted = if v == 0.0 {
                    current
                } else if axis.flag(wrap_axes(slot)) {
                    current + fling_travel(v, FLING_DECAY_PER_MS)
                } else {
                    (current + fling_travel(v, FLING_DECAY_PER_MS)).clamp(0.0, max)
                };
                let extent = axis.extent(slot.scrollport);
                let target = snap.settle(
                    axis.of(origin),
                    predicted,
                    SnapAxis::proximity_threshold(extent),
                );
                let distance = (target - current).abs();
                if distance < threshold {
                    v = 0.0;
                } else if v == 0.0 || distance <= extent {
                    self.glides.push(Glide::new(
                        slot.node,
                        axis,
                        current,
                        target,
                        v * 1000.0,
                        now,
                    ));
                    v = 0.0;
                } else {
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
            self.settle(frame, pointer, now);
            self.start_bounce_backs(frame, now);
            return;
        }
        self.flings.push(fling);
    }

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

    pub(super) fn tick(&mut self, frame: &CommittedFrame, now: f64) {
        if !self.is_animating() && self.auto_scrolls.is_empty() {
            return;
        }
        self.rebase(frame, now);
        self.tick_flings(frame, now);
        self.tick_bounce_backs(frame, now);
        self.tick_glides(frame, now);
        self.tick_auto_scrolls(frame, now);
        if !self.wheel_steps.is_empty() {
            self.tick_wheel_steps(now);
        }
    }

    #[must_use]
    pub(super) fn is_auto_scrolling(&self, frame: &CommittedFrame) -> bool {
        self.auto_scrolls.iter().any(|run| {
            self.auto_scroll_room(frame, run)
                .is_some_and(|room| room > 0.0)
        })
    }

    pub(super) fn is_auto_scrolling_node(&self, frame: &CommittedFrame, node: NodeId) -> bool {
        self.auto_scrolls.iter().any(|run| {
            run.node == node
                && self
                    .auto_scroll_room(frame, run)
                    .is_some_and(|room| room > 0.0)
        })
    }

    #[must_use]
    pub(super) fn has_auto_scroll(&self, node: NodeId) -> bool {
        self.auto_scrolls.iter().any(|run| run.node == node)
    }

    fn auto_scroll_room(&self, frame: &CommittedFrame, run: &AutoScroll) -> Option<f32> {
        let index = frame.slot_of(run.node)?;
        if self.is_held(run.node)
            || self.is_bouncing(run.node)
            || self.is_gliding(run.node)
            || self.is_flinging(frame, run.node)
        {
            return None;
        }
        let slot = &frame.scroll_slots()[index as usize];
        if run.axis.flag(wrap_axes(slot)) {
            return Some(f32::INFINITY);
        }
        let offset = run
            .axis
            .of(self.offsets.get(&run.node).copied().unwrap_or(slot.offset));
        let max = run.axis.of(slot.max_offset).max(0.0);
        let room = if run.rate > 0.0 { max - offset } else { offset };
        Some(room.max(0.0))
    }

    pub(super) fn apply_auto_scroll(&mut self, node: NodeId, request: AutoScrollRequest) {
        self.auto_scrolls.retain(|run| run.node != node);
        if request.rate != 0.0 {
            self.auto_scrolls.push(AutoScroll {
                node,
                axis: if request.horizontal { Axis::X } else { Axis::Y },
                rate: request.rate,
                stop_at_edge: request.stop_at_edge,
                since: None,
            });
        }
        note_changed(&mut self.changed, node);
    }

    fn tick_auto_scrolls(&mut self, frame: &CommittedFrame, now: f64) {
        if self.auto_scrolls.is_empty() {
            return;
        }
        let pixel = rest_threshold(frame.device_pixel_ratio());
        let mut runs = std::mem::take(&mut self.auto_scrolls);
        runs.retain_mut(|run| self.step_auto_scroll(frame, run, pixel, now));
        self.auto_scrolls = runs;
    }

    fn step_auto_scroll(
        &mut self,
        frame: &CommittedFrame,
        run: &mut AutoScroll,
        pixel: f32,
        now: f64,
    ) -> bool {
        let Some(room) = self.auto_scroll_room(frame, run) else {
            run.since = None;
            return true;
        };
        if room <= 0.0 {
            run.since = None;
            if run.stop_at_edge {
                self.end_auto_scroll(frame, run.node);
            }
            return !run.stop_at_edge;
        }
        let Some(since) = run.since else {
            run.since = Some(now);
            return true;
        };
        let elapsed = now - since;
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the seconds between two writes are well inside f32"
        )]
        let travel = run.rate.abs() * elapsed.max(0.0) as f32;
        if elapsed < AUTO_SCROLL_MIN_INTERVAL_SECONDS || travel < pixel {
            return true;
        }
        let Some(mut offset) = self.offset_of_slot(frame, run.node) else {
            return false;
        };
        let arrived = travel >= room;
        let position = run.axis.of(offset);
        run.axis.set(
            &mut offset,
            if !arrived {
                position + travel.copysign(run.rate)
            } else if run.rate > 0.0 {
                position + room
            } else {
                0.0
            },
        );
        self.set_offset(run.node, offset);
        run.since = Some(now);
        if arrived && run.stop_at_edge {
            self.end_auto_scroll(frame, run.node);
            return false;
        }
        true
    }

    fn end_auto_scroll(&mut self, frame: &CommittedFrame, node: NodeId) {
        note_changed(&mut self.changed, node);
        if let Some(index) = frame.slot_of(node) {
            self.settle_node_at_rest(frame, &frame.scroll_slots()[index as usize]);
        }
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
                self.settle(frame, fling.pointer, now);
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
                if !self.is_gliding(back.node) {
                    self.settle_node_at_rest(frame, slot);
                }
            } else {
                self.bounce_backs.push(back);
            }
        }
    }

    fn tick_glides(&mut self, frame: &CommittedFrame, now: f64) {
        let threshold = rest_threshold(frame.device_pixel_ratio());
        let mut landed: SmallVec<[NodeId; 2]> = SmallVec::new();
        for glide in std::mem::take(&mut self.glides) {
            let Some(mut offset) = self.offset_of_slot(frame, glide.node) else {
                continue;
            };
            let remaining = glide.remaining(now);
            let done = remaining.abs() < threshold;
            glide.axis.set(
                &mut offset,
                if done {
                    glide.target
                } else {
                    glide.target + remaining
                },
            );
            self.set_offset(glide.node, offset);
            if done {
                note_changed(&mut self.changed, glide.node);
                landed.push(glide.node);
            } else {
                self.glides.push(glide);
            }
        }
        for node in landed {
            if self.is_gliding(node) || self.is_held(node) || self.is_bouncing(node) {
                continue;
            }
            if let Some(index) = frame.slot_of(node) {
                self.settle_node_at_rest(frame, &frame.scroll_slots()[index as usize]);
            }
        }
    }

    #[expect(
        clippy::float_cmp,
        reason = "a request for exactly where a glide already goes changes nothing; one for a \
                  position any distance away starts over"
    )]
    pub(super) fn glide_to_or_keep(
        &mut self,
        frame: &CommittedFrame,
        node: NodeId,
        from: Vector2D<f32>,
        to: Vector2D<f32>,
        now: f64,
    ) {
        if !self.glides.is_empty() {
            self.glides
                .retain(|glide| glide.node != node || glide.target == glide.axis.of(to));
        }
        self.glide_to(frame, node, from, to, now);
    }

    pub(super) fn glide_to(
        &mut self,
        frame: &CommittedFrame,
        node: NodeId,
        from: Vector2D<f32>,
        to: Vector2D<f32>,
        now: f64,
    ) {
        let threshold = rest_threshold(frame.device_pixel_ratio());
        let mut written = from;
        for axis in Axis::BOTH {
            let gap = axis.of(to) - axis.of(from);
            if gap == 0.0 || self.is_gliding_axis(node, axis) {
                continue;
            }
            if gap.abs() < threshold {
                axis.set(&mut written, axis.of(to));
            } else {
                self.glides
                    .push(Glide::new(node, axis, axis.of(from), axis.of(to), 0.0, now));
            }
        }
        if written != from {
            self.write(node, written);
            self.generation += 1;
        }
    }

    pub(super) fn start_bounce_backs(&mut self, frame: &CommittedFrame, now: f64) {
        for slot in frame.scroll_slots() {
            if slot.bounce == ScrollAxes::NONE {
                continue;
            }
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
                    || self.is_gliding_axis(slot.node, axis)
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

    #[must_use]
    pub(super) fn is_held(&self, node: NodeId) -> bool {
        self.gesture_origins.keys().any(|(_, held)| *held == node)
    }

    #[must_use]
    pub(super) fn is_flinging(&self, frame: &CommittedFrame, node: NodeId) -> bool {
        self.flings
            .iter()
            .any(|fling| chain_nodes(frame, fling.from).contains(&node))
    }

    #[must_use]
    pub(super) fn reach_of(
        &self,
        frame: &CommittedFrame,
        slot: &crate::visual::ScrollSlot,
        offset: Vector2D<f32>,
    ) -> Vector2D<f32> {
        let node = slot.node;
        let fling = self
            .flings
            .iter()
            .find(|fling| chain_nodes(frame, fling.from).contains(&node));
        let wraps = wrap_axes(slot);
        let mut reach = Vector2D::zero();
        for axis in Axis::BOTH {
            if axis.flag(wraps) {
                continue;
            }
            let travel = match fling {
                Some(fling) => fling_travel(axis.of(fling.velocity), fling.rate(axis)),
                None => self
                    .glides
                    .iter()
                    .find(|glide| glide.node == node && glide.axis == axis)
                    .map_or(0.0, |glide| glide.target - axis.of(offset)),
            };
            let at = axis.of(offset);
            let landing = at + travel;
            let clamped = if landing.is_finite() {
                landing.clamp(0.0, axis.of(slot.max_offset).max(0.0))
            } else {
                at
            };
            axis.set(&mut reach, clamped - at);
        }
        reach
    }

    #[must_use]
    pub(super) fn is_bouncing(&self, node: NodeId) -> bool {
        self.bounce_backs.iter().any(|back| back.node == node)
    }

    #[must_use]
    pub(super) fn is_gliding(&self, node: NodeId) -> bool {
        self.glides.iter().any(|glide| glide.node == node)
    }

    fn is_gliding_axis(&self, node: NodeId, axis: Axis) -> bool {
        self.glides
            .iter()
            .any(|glide| glide.node == node && glide.axis == axis)
    }

    pub(super) fn stop_glides(&mut self, node: NodeId) {
        if !self.glides.is_empty() {
            self.glides.retain(|glide| glide.node != node);
        }
    }

    pub(super) fn stop_flings_and_bounces_on(
        &mut self,
        frame: &CommittedFrame,
        node: NodeId,
        now: f64,
    ) {
        let mut stopped: SmallVec<[PointerId; 1]> = SmallVec::new();
        self.flings.retain(|fling| {
            let through = chain_nodes(frame, fling.from).contains(&node);
            if through {
                stopped.push(fling.pointer);
            }
            !through
        });
        if !stopped.is_empty() {
            for pointer in stopped {
                self.gesture_origins.remove(&(pointer, node));
                self.settle(frame, pointer, now);
            }
            self.start_bounce_backs(frame, now);
        }
        self.bounce_backs.retain(|back| back.node != node);
    }

    pub(super) fn shift_motion(&mut self, node: NodeId, shift: Vector2D<f32>) {
        for ((_, held), origin) in &mut self.gesture_origins {
            if *held == node {
                *origin += shift;
            }
        }
        for glide in &mut self.glides {
            if glide.node == node {
                glide.target += glide.axis.of(shift);
            }
        }
    }

    pub(super) fn retain_motion(&mut self, frame: &CommittedFrame) {
        self.flings
            .retain(|fling| frame.slot_of(fling.from).is_some());
        self.glides.retain_mut(|glide| {
            let Some(index) = frame.slot_of(glide.node) else {
                return false;
            };
            let slot = &frame.scroll_slots()[index as usize];
            if !glide.axis.flag(wrap_axes(slot)) {
                let max = glide.axis.of(slot.max_offset);
                glide.target = glide.target.clamp(0.0, max.max(0.0));
            }
            true
        });
        self.auto_scrolls
            .retain(|run| frame.slot_of(run.node).is_some());
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
