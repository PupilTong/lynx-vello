//! Scroll-driven animations (scroll-animations-1): the main thread's half.
//!
//! A CSS animation whose `animation-timeline` is not `auto` is
//! progress-driven (the fork's `Animation::is_progress_driven`): its effect
//! is a pure function of one scroll container's offset along one axis.
//! Stylo evaluates no timeline; it cascades `Animation::timeline_sample`,
//! which this module writes.
//!
//! - [`ProgressTiming`] and [`iteration_progress`] are that function: the animation's range, delay
//!   and duration normalized into scroll-offset px once per commit, and web-animations-1's phase
//!   and iteration arithmetic applied to an offset.
//! - [`Document::resolve_timelines`] binds every progress-driven animation to its timeline after a
//!   layout pass and writes its sample. A change re-cascades the element inside the same `layout()`
//!   call — scroll-animations-1 §5.1's stale-timelines pass — so the commit that creates an
//!   animation already shows it at its offset.
//! - [`Document::advance_scroll_timelines`] re-samples, between commits, the animations a moved
//!   scroll container drives.
//!
//! Where the specifications are silent or disagree this follows Blink;
//! `docs/tracking/css-animation.md` lists each choice.

use euclid::default::{Size2D, Vector2D};
use hughie::style::PositionProperty;
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use stylo::dom::OpaqueNode;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::UI;
use stylo::rule_tree::{CascadeLevel, ShadowCascadeOrder};
use stylo::servo::animation::{Animation, AnimationProgress, AnimationSetKey, AnimationState};
use stylo::values::computed::{
    AnimationDirection, AnimationFillMode, AnimationTimeline, Length, LengthPercentage, ScrollAxis,
    TimelineName, ViewTimelineInset,
};
use stylo::values::generics::animation::AnimationRangeValue;
use stylo::values::generics::length::LengthPercentageOrAuto;
use stylo::values::specified::animation::{Scroller, TimelineRangeName};
use stylo_atoms::Atom;

use crate::layout::{DisplayMode, display_mode};
use crate::scroll::snap::scroll_padding;
use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeId};
use crate::visual::sticky::{self, StickyAxis};

/// A physical scroll axis. The engine lays out `horizontal-tb` only, so
/// `block` is `y` and `inline` is `x`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Y,
}

impl Axis {
    const fn of(axis: ScrollAxis) -> Self {
        match axis {
            ScrollAxis::Block | ScrollAxis::Y => Self::Y,
            ScrollAxis::Inline | ScrollAxis::X => Self::X,
        }
    }

    const fn of_vector(self, vector: Vector2D<f32>) -> f32 {
        match self {
            Self::X => vector.x,
            Self::Y => vector.y,
        }
    }

    const fn of_size(self, size: Size2D<f32>) -> f32 {
        match self {
            Self::X => size.width,
            Self::Y => size.height,
        }
    }
}

/// One progress-driven animation's timing over its timeline, in scroll-offset
/// px: web-animations-1's timing model with the animation's start time
/// aligned to its range start (web-animations-2 §2.3.7.1) and a playback
/// rate of 1. The phase boundaries are absolute offsets, so an offset at the
/// range's end compares equal to it exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProgressTiming {
    /// The attachment range's start (`animation-range-start` resolved): local
    /// time 0.
    pub start: f32,
    /// Where the active time is 0: `start` plus the start delay, converted
    /// proportionally into the range.
    pub origin: f64,
    /// Where the active interval ends: the attachment range's end, or
    /// `start` when the active duration is 0.
    pub end: f32,
    /// The iteration duration: 0 when the duration or the count is 0, the
    /// count is infinite, the duration and delay total nothing, or the range
    /// is empty.
    pub iteration: f64,
    /// The largest offset the source reaches, the timeline's maximum time
    /// whatever its own 0% and 100% are.
    pub limit: f32,
    pub iterations: f64,
    pub direction: AnimationDirection,
    pub fill: AnimationFillMode,
}

impl ProgressTiming {
    /// The timing of an animation attached to `range` of a timeline whose
    /// source scrolls to `limit`, with `duration` `None` for `auto`.
    ///
    /// Blink's proportional conversion (`AnimationEffect::NormalizedTiming`):
    /// css-animations-2 treats a time duration as `auto` on a progress
    /// timeline and web-animations-2 converts it into proportions, and the
    /// two agree unless a delay is non-zero. `auto` fills the range and takes
    /// no delay; a time duration and delay keep their proportions to each
    /// other across the range. An infinite count or a duration and delay
    /// totalling nothing leave a zero active duration at the range's start.
    /// The end delay is always 0 in CSS, so a non-zero active duration always
    /// ends at the range's end.
    pub(crate) fn normalized(
        limit: f32,
        range: [f32; 2],
        duration: Option<f64>,
        delay: f64,
        iterations: f64,
        direction: AnimationDirection,
        fill: AnimationFillMode,
    ) -> Self {
        let length = f64::from(range[1]) - f64::from(range[0]);
        let counted = length > 0.0 && iterations.is_finite() && iterations > 0.0;
        // The start delay and the iteration duration, for a non-zero end time.
        let proportions = match duration {
            _ if !counted => None,
            None => Some((0.0, length / iterations)),
            Some(duration) => {
                let total = delay + duration * iterations;
                (total > 0.0).then(|| (delay * length / total, duration * length / total))
            }
        };
        let (start_delay, iteration, end) = proportions
            .map_or((0.0, 0.0, range[0]), |(delay, iteration)| {
                (delay, iteration, range[1])
            });
        Self {
            start: range[0],
            origin: f64::from(range[0]) + start_delay,
            end,
            iteration,
            limit,
            iterations,
            direction,
            fill,
        }
    }

    /// The active duration.
    fn active(&self) -> f64 {
        (f64::from(self.end) - self.origin).max(0.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Before,
    Active,
    After,
}

/// Where the animation `timing` describes stands at scroll `offset`: its
/// simple iteration progress and whether that iteration runs reversed, the
/// only input `Animation::sample_at` takes; `None` when it has no effect
/// (before or after its active interval without the matching fill).
///
/// web-animations-1 §4.5-4.8 verbatim for a playback rate of 1, with
/// web-animations-2 §2.4.4's progress-timeline exception: at the timeline's
/// maximum time — the source's scroll limit, whatever the timeline's own
/// 100% is — an active interval ending there is still active, so it shows
/// its last keyframe without a forwards fill. The direction is left to
/// `sample_at`, which applies it itself; a backwards-filled before phase
/// answers `AnimationProgress::before_phase`, which `sample_at` reads as the
/// first keyframe without easing it, as stylo's time path does.
#[expect(
    clippy::float_cmp,
    reason = "web-animations compares boundaries and progress exactly, and so does the painter"
)]
pub(crate) fn iteration_progress(
    timing: &ProgressTiming,
    offset: f32,
) -> Option<AnimationProgress> {
    let active = timing.active();
    let phase = if f64::from(offset) < timing.origin.max(f64::from(timing.start)) {
        Phase::Before
    } else if offset > timing.end || (offset == timing.end && offset != timing.limit) {
        Phase::After
    } else {
        Phase::Active
    };
    let backwards = matches!(
        timing.fill,
        AnimationFillMode::Backwards | AnimationFillMode::Both
    );
    let forwards = matches!(
        timing.fill,
        AnimationFillMode::Forwards | AnimationFillMode::Both
    );
    match phase {
        Phase::Before if !backwards => return None,
        Phase::After if !forwards => return None,
        _ => {}
    }
    let active_time = (f64::from(offset) - timing.origin).clamp(0.0, active);
    let iterations = timing.iterations;
    // At the end of the active interval the overall progress is the count
    // itself, which dividing by a rounded iteration duration can miss.
    let ended = phase != Phase::Before && active_time == active;
    let overall = if timing.iteration == 0.0 {
        if phase == Phase::Before {
            0.0
        } else {
            iterations
        }
    } else if ended {
        iterations
    } else {
        (active_time / timing.iteration).min(iterations)
    };
    let mut simple = if overall.is_infinite() {
        0.0
    } else {
        overall % 1.0
    };
    if simple == 0.0 && ended && iterations != 0.0 {
        simple = 1.0;
    }
    let current = if phase == Phase::After && iterations.is_infinite() {
        f64::INFINITY
    } else if simple == 1.0 {
        overall.floor() - 1.0
    } else {
        overall.floor()
    };
    let reversed = match timing.direction {
        AnimationDirection::Normal => false,
        AnimationDirection::Reverse => true,
        AnimationDirection::Alternate | AnimationDirection::AlternateReverse => {
            let turn = if timing.direction == AnimationDirection::AlternateReverse {
                current + 1.0
            } else {
                current
            };
            turn.is_finite() && turn % 2.0 != 0.0
        }
    };
    Some(if phase == Phase::Before && active_time == 0.0 {
        AnimationProgress::before_phase(reversed)
    } else {
        AnimationProgress::new(simple, reversed)
    })
}

/// What one progress-driven animation is attached to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Binding {
    /// An active timeline: `source`'s offset along `axis`, read through
    /// `timing`.
    Active {
        source: NodeId,
        axis: Axis,
        timing: ProgressTiming,
    },
    /// No timeline, or an inactive one: no effect, and the animation is not
    /// current (an animation on an inactive timeline is idle).
    Inactive,
}

impl Binding {
    const fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    fn sample(&self, offset_of: impl Fn(NodeId) -> Vector2D<f32>) -> Option<AnimationProgress> {
        match self {
            Self::Active {
                source,
                axis,
                timing,
            } => iteration_progress(timing, axis.of_vector(offset_of(*source))),
            Self::Inactive => None,
        }
    }
}

/// One entry of the binding table.
#[derive(Debug)]
struct Bound {
    binding: Binding,
    /// Whether a sample was written, so a paused animation holds only once
    /// it has one.
    sampled: bool,
}

/// The timelines side of the animation driver, sized by animations and
/// timeline declarations, never by the page.
#[derive(Default)]
pub(crate) struct ScrollTimelines {
    /// The elements whose set holds a progress-driven animation, rebuilt by
    /// `Document::sync_animation_state`.
    pub(crate) progress_driven: FxHashSet<NodeId>,
    /// Each progress-driven animation's binding, keyed by element and
    /// animation name, from the last [`Document::resolve_timelines`]: a set
    /// holds one live animation per name (stylo's `return;` deviation), and
    /// its index moves when an earlier one leaves.
    bindings: FxHashMap<(NodeId, Atom), Bound>,
    /// The table the next resolution builds into.
    spare: FxHashMap<(NodeId, Atom), Bound>,
    /// Scroll container → the animations an active binding reads it for.
    dependents: FxHashMap<NodeId, SmallVec<[(NodeId, Atom); 2]>>,
    /// Timeline name → the elements whose `scroll-timeline-name` or
    /// `view-timeline-name` lists it, kept from the flush's restyles.
    definers: FxHashMap<Atom, SmallVec<[NodeId; 2]>>,
    /// Element → the names it is listed under in [`Self::definers`].
    defined: FxHashMap<NodeId, SmallVec<[Atom; 2]>>,
    /// The elements whose `timeline-scope` is not `none`.
    scopers: FxHashSet<NodeId>,
}

impl ScrollTimelines {
    /// The binding `id`'s animation `name` resolved to.
    #[cfg(test)]
    pub(crate) fn binding(&self, id: NodeId, name: &str) -> Option<&Binding> {
        self.bindings
            .get(&(id, Atom::from(name)))
            .map(|bound| &bound.binding)
    }

    /// Whether `id` is in the definer table.
    #[cfg(test)]
    pub(crate) fn defines(&self, id: NodeId) -> bool {
        self.defined.contains_key(&id) || self.scopers.contains(&id)
    }

    pub(crate) fn has_definers(&self) -> bool {
        !self.defined.is_empty() || !self.scopers.is_empty()
    }

    /// web-animations-1's *current* for `id`'s animation `name`, as far as
    /// its timeline decides it: a binding not resolved yet counts as active
    /// until the resolution the same `layout()` call runs.
    pub(crate) fn is_current(&self, id: NodeId, name: &Atom) -> bool {
        self.bindings
            .get(&(id, name.clone()))
            .is_none_or(|bound| bound.binding.is_active())
    }

    /// Re-reads which timeline names `id` defines and whether it scopes any,
    /// from its restyled `style` (`None` when it has none). An element that
    /// neither defined nor defines one costs three style reads and two
    /// probes.
    pub(crate) fn restyled(&mut self, id: NodeId, style: Option<&ComputedValues>) {
        let defines = style.is_some_and(|style| {
            let ui = style.get_ui();
            ui.specifies_scroll_timelines()
                || ui.specifies_view_timelines()
                || ui.specifies_timeline_scope()
        });
        if !defines && !self.defined.contains_key(&id) && !self.scopers.contains(&id) {
            return;
        }
        self.undefine(id);
        let Some(ui) = style.map(|style| style.get_ui()) else {
            return;
        };
        let mut names: SmallVec<[Atom; 2]> = SmallVec::new();
        for name in ui
            .scroll_timeline_name_iter()
            .chain(ui.view_timeline_name_iter())
        {
            if let Some(atom) = name.value.as_atom()
                && !names.contains(atom)
            {
                names.push(atom.clone());
            }
        }
        for name in &names {
            self.definers.entry(name.clone()).or_default().push(id);
        }
        if !names.is_empty() {
            self.defined.insert(id, names);
        }
        if ui.specifies_timeline_scope() {
            self.scopers.insert(id);
        }
    }

    fn undefine(&mut self, id: NodeId) {
        if let Some(names) = self.defined.remove(&id) {
            for name in names {
                if let Some(list) = self.definers.get_mut(&name) {
                    list.retain(|definer| *definer != id);
                    if list.is_empty() {
                        self.definers.remove(&name);
                    }
                }
            }
        }
        self.scopers.remove(&id);
    }

    pub(crate) fn forget(&mut self, ids: &[NodeId]) {
        for &id in ids {
            self.undefine(id);
            self.progress_driven.remove(&id);
        }
        self.bindings.retain(|(id, _), _| !ids.contains(id));
        for list in self.dependents.values_mut() {
            list.retain(|(id, _)| !ids.contains(id));
        }
    }
}

/// A timeline a name or a function resolved to, before its geometry.
enum Timeline {
    Scroll {
        source: NodeId,
        axis: Axis,
    },
    View {
        subject: NodeId,
        axis: Axis,
        inset: ViewTimelineInset,
    },
}

/// The timeline an element's own style defines under a name.
enum Declared {
    Scroll(ScrollAxis),
    View(ScrollAxis, ViewTimelineInset),
}

/// A timeline name as one element references it, resolved to the tree it
/// is scoped to.
struct Reference<'a> {
    atom: &'a Atom,
    /// The shadow root, or `None` for the document tree.
    tree: Option<NodeId>,
}

/// A view progress timeline's named ranges, in scroll-offset px.
struct ViewRanges {
    cover: [f32; 2],
    contain: [f32; 2],
    entry: [f32; 2],
    exit: [f32; 2],
    entry_crossing: [f32; 2],
    exit_crossing: [f32; 2],
    scroll: [f32; 2],
}

/// One end of the attachment range: `normal` is the timeline's own end, a
/// bare `<length-percentage>` measures from the timeline's start against its
/// length, and a named range on a view timeline measures from that range's
/// start against its length. A range name on a scroll timeline names the
/// whole timeline, as Blink resolves it.
fn range_point(
    value: &AnimationRangeValue<LengthPercentage>,
    end: bool,
    bounds: [f32; 2],
    ranges: Option<&ViewRanges>,
) -> f32 {
    let [from, to] = match (value.name, ranges) {
        (TimelineRangeName::Normal, _) => return bounds[usize::from(end)],
        (TimelineRangeName::None, _) | (_, None) => bounds,
        (TimelineRangeName::Cover, Some(ranges)) => ranges.cover,
        (TimelineRangeName::Contain, Some(ranges)) => ranges.contain,
        (TimelineRangeName::Entry, Some(ranges)) => ranges.entry,
        (TimelineRangeName::Exit, Some(ranges)) => ranges.exit,
        (TimelineRangeName::EntryCrossing, Some(ranges)) => ranges.entry_crossing,
        (TimelineRangeName::ExitCrossing, Some(ranges)) => ranges.exit_crossing,
        (TimelineRangeName::Scroll, Some(ranges)) => ranges.scroll,
    };
    from + value.lp.resolve(Length::new(to - from)).px()
}

/// Animation `index` of `ui`'s timing over a timeline spanning `bounds`,
/// whose source scrolls to `limit`.
fn timing(
    ui: &UI,
    index: usize,
    bounds: [f32; 2],
    limit: f32,
    ranges: Option<&ViewRanges>,
) -> ProgressTiming {
    let start = range_point(
        &ui.animation_range_start_mod(index).0,
        false,
        bounds,
        ranges,
    );
    let end = range_point(&ui.animation_range_end_mod(index).0, true, bounds, ranges);
    let duration = ui.animation_duration_mod(index);
    ProgressTiming::normalized(
        limit,
        [start, end],
        (!duration.is_auto()).then(|| f64::from(duration.seconds())),
        f64::from(ui.animation_delay_mod(index).seconds()),
        f64::from(ui.animation_iteration_count_mod(index).0),
        ui.animation_direction_mod(index),
        ui.animation_fill_mode_mod(index),
    )
}

/// A continuous piecewise-linear function of the scroll offset, linear
/// between its knots and constant outside them: the summed sticky shift of
/// a view-timeline subject along the timeline's axis. No knots is 0.
#[derive(Default)]
struct Knots(SmallVec<[(f32, f32); 8]>);

impl Knots {
    fn at(&self, offset: f32) -> f32 {
        let knots = &self.0;
        let Some(&(first, value)) = knots.first() else {
            return 0.0;
        };
        if offset <= first {
            return value;
        }
        for pair in knots.windows(2) {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            if offset <= x1 {
                return y0 + (offset - x0) * (y1 - y0) / (x1 - x0);
            }
        }
        knots[knots.len() - 1].1
    }

    /// The offsets `o` with `o - self.at(o) == target`, as `(earliest,
    /// latest)`. `o - self.at(o)` never decreases, with slope 1 outside the
    /// knots, so they form one closed interval.
    #[expect(clippy::float_cmp, reason = "a flat segment is exactly flat")]
    fn preimage(&self, target: f32) -> (f32, f32) {
        let knots = &self.0;
        let (Some(&(first, first_value)), Some(&(last, last_value))) =
            (knots.first(), knots.last())
        else {
            return (target, target);
        };
        let (mut earliest, mut latest) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut hit = |offset: f32| {
            earliest = earliest.min(offset);
            latest = latest.max(offset);
        };
        if target <= first - first_value {
            hit(target + first_value);
        }
        if target >= last - last_value {
            hit(target + last_value);
        }
        for pair in knots.windows(2) {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            let (v0, v1) = (x0 - y0, x1 - y1);
            if v0 <= target && target <= v1 {
                if v1 == v0 {
                    hit(x0);
                    hit(x1);
                } else {
                    hit(x0 + (target - v0) * (x1 - x0) / (v1 - v0));
                }
            }
        }
        (earliest, latest)
    }

    /// This shift plus that of a sticky box inside every box it sums, which
    /// slides by `sticky.offset(o, self.at(o))`: evaluated on every knot of
    /// both, which is exact because the sum is linear between them.
    fn nest(&self, sticky: StickyAxis) -> Self {
        let mut offsets: SmallVec<[f32; 16]> = self.0.iter().map(|&(offset, _)| offset).collect();
        for kink in sticky.kinks() {
            let (earliest, latest) = self.preimage(kink);
            offsets.extend([earliest, latest]);
        }
        offsets.sort_by(f32::total_cmp);
        offsets.dedup();
        Self(
            offsets
                .into_iter()
                .map(|offset| {
                    let inherited = self.at(offset);
                    (offset, inherited + sticky.offset(offset, inherited))
                })
                .collect(),
        )
    }
}

impl<T: Sync> Document<T> {
    /// Binds every progress-driven animation to its timeline against the
    /// layout just committed, writes its sample, and re-cascades the
    /// elements whose sample or binding changed. Answers whether that left
    /// layout work, which `layout()` takes as one more pass.
    ///
    /// The timeline kind and the timeline come from `Animation::timeline`,
    /// what stylo captured when it started or updated the animation; the
    /// style is read by the animation's name only for its range, duration,
    /// delay, fill, count and direction. A paused animation holds its sample
    /// once it has one, and an element in a skipped subtree holds whatever it
    /// had (css-contain-2 §4).
    pub(crate) fn resolve_timelines(&mut self) -> bool {
        let timelines = &mut self.animations_mut().timelines;
        if timelines.progress_driven.is_empty() && timelines.bindings.is_empty() {
            return false;
        }
        let mut previous = std::mem::take(&mut timelines.bindings);
        let mut bindings = std::mem::take(&mut timelines.spare);
        let mut dependents = std::mem::take(&mut timelines.dependents);
        let progress_driven = std::mem::take(&mut timelines.progress_driven);
        dependents.clear();
        let mut hinted = Vec::new();
        let mut flipped = false;
        {
            let handle = self.animations().context_handle();
            let mut sets = handle.sets.write();
            let document: &Self = self;
            for &id in &progress_driven {
                let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(id.arena_key()));
                let Some(set) = sets.get_mut(&key) else {
                    continue;
                };
                let skipped = document.in_skipped_subtree(id);
                let mut changed = false;
                for animation in &mut set.animations {
                    if !animation.is_progress_driven()
                        || animation.state == AnimationState::Canceled
                    {
                        continue;
                    }
                    let binding = document.bind(id, animation);
                    let key = (id, animation.name.clone());
                    let old = previous.remove(&key);
                    let mut sampled = old.as_ref().is_some_and(|bound| bound.sampled);
                    // An unresolved binding counted as active.
                    let was_active = old.as_ref().is_none_or(|bound| bound.binding.is_active());
                    flipped |= was_active != binding.is_active();
                    changed |= old.is_none_or(|bound| bound.binding != binding);
                    let paused = matches!(animation.state, AnimationState::Paused(_));
                    if !skipped && (!paused || !sampled) {
                        sampled = true;
                        let sample = binding.sample(|source| document.scroll_offset(source));
                        if animation.timeline_sample != sample {
                            animation.timeline_sample = sample;
                            changed = true;
                        }
                    }
                    if let Binding::Active { source, .. } = binding {
                        dependents.entry(source).or_default().push(key.clone());
                    }
                    bindings.insert(key, Bound { binding, sampled });
                }
                if changed && !skipped {
                    hinted.push(id);
                }
            }
        }
        previous.clear();
        let timelines = &mut self.animations_mut().timelines;
        timelines.bindings = bindings;
        timelines.spare = previous;
        timelines.dependents = dependents;
        timelines.progress_driven = progress_driven;
        if hinted.is_empty() && !flipped {
            return false;
        }
        let relayout = self.recascade_timeline_dependents(&hinted);
        // The re-cascade and an activity flip both move what the map
        // settles to: the `animates` bits read each binding's activity.
        relayout | self.sync_animation_state()
    }

    /// Re-samples the animations whose timeline reads one of `sources`, after
    /// their offsets moved without a commit — the painter's scroll adopted by
    /// the main thread — and re-cascades the elements whose sample changed.
    /// A paused animation holds, and so does one in a skipped subtree.
    ///
    /// Every dependent is re-sampled: an element holding a progress-driven
    /// animation exports no curve, so none is covered by the painter.
    pub fn advance_scroll_timelines(&mut self, sources: &[NodeId]) {
        if self.animations().timelines.dependents.is_empty() {
            return;
        }
        let mut hinted: SmallVec<[NodeId; 4]> = SmallVec::new();
        {
            let handle = self.animations().context_handle();
            let mut sets = handle.sets.write();
            let timelines = &self.animations().timelines;
            for source in sources {
                for key in timelines.dependents.get(source).into_iter().flatten() {
                    let Some(bound) = timelines.bindings.get(key) else {
                        continue;
                    };
                    let (id, name) = (key.0, &key.1);
                    let set_key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(id.arena_key()));
                    let Some(animation) = sets.get_mut(&set_key).and_then(|set| {
                        set.animations.iter_mut().find(|animation| {
                            animation.name == *name
                                && animation.is_progress_driven()
                                && animation.state != AnimationState::Canceled
                        })
                    }) else {
                        continue;
                    };
                    if !matches!(animation.state, AnimationState::Running)
                        || self.in_skipped_subtree(id)
                    {
                        continue;
                    }
                    let sample = bound.binding.sample(|source| self.scroll_offset(source));
                    if animation.timeline_sample != sample {
                        animation.timeline_sample = sample;
                        if !hinted.contains(&id) {
                            hinted.push(id);
                        }
                    }
                }
            }
        }
        if !hinted.is_empty() {
            self.recascade_timeline_dependents(&hinted);
            self.sync_animation_state();
        }
    }

    /// Re-cascades `hinted` for their new samples, answering whether that
    /// left layout work.
    fn recascade_timeline_dependents(&mut self, hinted: &[NodeId]) -> bool {
        if hinted.is_empty() {
            return false;
        }
        let root = self.hint_animated_elements(hinted);
        let tick = self.recascade_animated_elements(root);
        if tick.restyled > 0 {
            self.note_visual_mutation();
        }
        tick.relayout
    }

    /// What `animation` of `id` is attached to, against the current layout.
    fn bind(&self, id: NodeId, animation: &Animation) -> Binding {
        let Some(ui) = self.paint_style(id).map(|style| style.get_ui()) else {
            return Binding::Inactive;
        };
        let Some(index) = ui
            .animation_name_iter()
            .position(|name| name.as_atom() == Some(&animation.name))
        else {
            return Binding::Inactive;
        };
        let timeline = match &animation.timeline {
            AnimationTimeline::Auto => None,
            AnimationTimeline::Timeline(name) => self.named_timeline(id, name),
            AnimationTimeline::Scroll(scroll) => match scroll.scroller {
                Scroller::Nearest => self
                    .nearest_scroll_container(id)
                    .or_else(|| self.root_scroller()),
                Scroller::Root => self.root_scroller(),
                Scroller::SelfElement => Some(id).filter(|&id| self.is_scroll_container(id)),
            }
            .map(|source| Timeline::Scroll {
                source,
                axis: Axis::of(scroll.axis),
            }),
            AnimationTimeline::View(view) => Some(Timeline::View {
                subject: id,
                axis: Axis::of(view.axis),
                inset: view.inset.clone(),
            }),
        };
        timeline
            .and_then(|timeline| self.bind_to(&timeline, ui, index))
            .unwrap_or(Binding::Inactive)
    }

    /// `scroll(root)`, and `scroll(nearest)` with no scroll container above:
    /// the document element stands in for the viewport, and is inactive
    /// unless it is a scroll container (the engine has no viewport
    /// scrolling area).
    fn root_scroller(&self) -> Option<NodeId> {
        Some(DOCUMENT_ELEMENT_NODE_ID).filter(|&root| self.is_scroll_container(root))
    }

    /// `timeline` with animation `index` of `ui`'s timing, or `None` when it
    /// is inactive: its source cannot scroll along its axis, its view
    /// subject has no box, or its cover range is empty.
    fn bind_to(&self, timeline: &Timeline, ui: &UI, index: usize) -> Option<Binding> {
        let (source, axis, bounds, limit, ranges) = match timeline {
            &Timeline::Scroll { source, axis } => {
                let max = axis.of_vector(self.scroll_box(source)?.max_offset());
                (source, axis, [0.0, max], max, None)
            }
            Timeline::View {
                subject,
                axis,
                inset,
            } => {
                let source = self.nearest_scroll_container(*subject)?;
                let ranges = self.view_ranges(*subject, source, *axis, inset)?;
                (source, *axis, ranges.cover, ranges.scroll[1], Some(ranges))
            }
        };
        (bounds[1] > bounds[0]).then(|| Binding::Active {
            source,
            axis,
            timing: timing(ui, index, bounds, limit, ranges.as_ref()),
        })
    }

    /// The named ranges of `subject`'s view progress timeline over `source`
    /// (scroll-animations-1 §3.1): where its border box, unscrolled and
    /// untransformed, meets the scrollport inset by `inset`, whose `auto`
    /// sides are the source's `scroll-padding`.
    ///
    /// A sticky box between the subject and its source moves the subject
    /// with the offset, so each edge alignment can hold over an interval of
    /// offsets; each range takes the earliest or latest end the spec names.
    /// Blink handles only the first sticky container; this solves the chain.
    fn view_ranges(
        &self,
        subject: NodeId,
        source: NodeId,
        axis: Axis,
        inset: &ViewTimelineInset,
    ) -> Option<ViewRanges> {
        // No box: `display: none` or `contents`, or skipped contents, whose
        // layout is zeroed.
        let style = self.paint_style(subject)?;
        if matches!(
            display_mode(style.clone_display()),
            DisplayMode::None | DisplayMode::Contents
        ) || self.in_skipped_subtree(subject)
        {
            return None;
        }
        let scroll_box = self.scroll_box(source)?;
        let max = axis.of_vector(scroll_box.max_offset());
        if max <= 0.0 {
            return None;
        }
        let rect = self.rect_in_scroll_container(subject, source)?;
        let (position, length) = (
            axis.of_vector(rect.origin.to_vector()),
            axis.of_size(rect.size),
        );
        let port = axis.of_size(scroll_box.scrollport);
        let padding = self.paint_style(source)?;
        let (padding_start, padding_end) = match axis {
            Axis::X => (
                padding.clone_scroll_padding_left(),
                padding.clone_scroll_padding_right(),
            ),
            Axis::Y => (
                padding.clone_scroll_padding_top(),
                padding.clone_scroll_padding_bottom(),
            ),
        };
        let resolve = |side: &LengthPercentageOrAuto<LengthPercentage>, padding| match side {
            LengthPercentageOrAuto::Auto => scroll_padding(padding, port),
            LengthPercentageOrAuto::LengthPercentage(length) => {
                length.resolve(Length::new(port)).px()
            }
        };
        let (inset_start, inset_end) = (
            resolve(&inset.start, padding_start),
            resolve(&inset.end, padding_end),
        );
        let shift = self.sticky_shift(subject, source, axis);
        // The start edge at the visibility range's end, the end edge there,
        // the start edge at its start, the end edge there.
        let a = shift.preimage(position - (port - inset_end));
        let b = shift.preimage(position + length - (port - inset_end));
        let c = shift.preimage(position - inset_start);
        let d = shift.preimage(position + length - inset_start);
        let contain = [b.0.min(c.0), b.1.max(c.1)];
        Some(ViewRanges {
            cover: [a.1, d.0],
            contain,
            entry: [a.1, contain[0]],
            exit: [contain[1], d.0],
            entry_crossing: [a.1, b.0],
            exit_crossing: [c.1, d.0],
            scroll: [0.0, max],
        })
    }

    /// The summed shift of the sticky boxes from `subject` up to `source` on
    /// its containing-block chain, as a function of `source`'s offset.
    /// `source` is the chain's nearest scroll container, and every scroll
    /// container scrolls both axes, so it is every such box's scroller.
    fn sticky_shift(&self, subject: NodeId, source: NodeId, axis: Axis) -> Knots {
        let mut chain: SmallVec<[NodeId; 2]> = SmallVec::new();
        let mut current = Some(subject);
        while let Some(id) = current
            && id != source
        {
            if self
                .paint_style(id)
                .is_some_and(|style| style.clone_position() == PositionProperty::Sticky)
            {
                chain.push(id);
            }
            current = self.scroll_parent(id);
        }
        chain.iter().rev().fold(Knots::default(), |shift, &id| {
            let [x, y] = sticky::axes(self, id, Some(source), Some(source));
            shift.nest(match axis {
                Axis::X => x,
                Axis::Y => y,
            })
        })
    }

    /// scroll-animations-1 §4.2's lookup of `name` for `id`, over the flat
    /// tree: the nearest inclusive ancestor defining it; else, at the
    /// nearest one whose `timeline-scope` limits it or at the document
    /// element, the last of its descendants in flat tree order that defines
    /// it, skipping those a nested `timeline-scope` limits already.
    fn named_timeline(&self, id: NodeId, name: &TimelineName) -> Option<Timeline> {
        let reference = Reference {
            atom: name.value.as_atom()?,
            tree: self.name_tree(id, name.scope),
        };
        let mut current = Some(id);
        while let Some(scope) = current {
            let node = self.get(scope)?;
            if let Some(style) = node.layout_computed_style() {
                if let Some(declared) = self.declared(scope, style.get_ui(), &reference) {
                    return self.defined_timeline(scope, declared);
                }
                if scope == DOCUMENT_ELEMENT_NODE_ID || self.limits(scope, style, &reference) {
                    let definer = self.last_definer_under(scope, &reference)?;
                    let declared =
                        self.declared(definer, self.paint_style(definer)?.get_ui(), &reference)?;
                    return self.defined_timeline(definer, declared);
                }
            }
            current = node.flat_parent_id();
        }
        None
    }

    /// The tree a tree-scoped name cascaded for `element` at `scope` belongs
    /// to: a shadow root, or `None` for the document tree. Stylo records the
    /// scope relative to the element (css-scoping §3.5), and this reads it
    /// back the way stylo's rule collector writes it:
    /// - 0: the element's own tree;
    /// - `-j`: `::slotted` rules from the tree of the `j`-th slot outward along the element's
    ///   assigned-slot chain, and `-(1 + the chain's length)`: `:host` rules from the element's own
    ///   shadow tree;
    /// - `n > 0`: `::part` rules from the `n`-th tree outward that has any `::part` rule, the trees
    ///   without one not counted.
    fn name_tree(&self, element: NodeId, scope: CascadeLevel) -> Option<NodeId> {
        let own = self.containing_shadow_root(element);
        let order = scope.shadow_order();
        let same = ShadowCascadeOrder::for_same_tree();
        if !scope.is_tree() || order == same {
            return own;
        }
        if order < same {
            let mut step = ShadowCascadeOrder::for_outermost_shadow_tree();
            let mut slot = self.assigned_slot(element);
            while let Some(current) = slot {
                if step == order {
                    return self.containing_shadow_root(current);
                }
                step.dec();
                slot = self.assigned_slot(current);
            }
            return self.shadow_root(element);
        }
        // Past the last shadow tree is the document tree.
        let mut tree = own?;
        let mut step = ShadowCascadeOrder::for_innermost_containing_tree();
        loop {
            tree = self.containing_shadow_root(self.shadow_host(tree)?)?;
            let has_part_rules = self
                .get(tree)
                .and_then(|root| root.shadow_data())
                .is_some_and(|shadow| shadow.styles.data.part_rules(&[]).is_some());
            if has_part_rules {
                if step >= order {
                    return Some(tree);
                }
                step.inc();
            }
        }
    }

    /// Whether `name`, cascaded for `element`, is `reference`.
    fn names(&self, element: NodeId, name: &TimelineName, reference: &Reference<'_>) -> bool {
        name.value.as_atom() == Some(reference.atom)
            && self.name_tree(element, name.scope) == reference.tree
    }

    /// The timeline `element`'s `ui` defines under `reference`: later entries
    /// of a list win, and a scroll timeline beats a view timeline
    /// (scroll-animations-1 §4.2).
    fn declared(&self, element: NodeId, ui: &UI, reference: &Reference<'_>) -> Option<Declared> {
        let last = |names: &mut dyn Iterator<Item = TimelineName>| {
            names
                .enumerate()
                .filter(|(_, name)| self.names(element, name, reference))
                .map(|(index, _)| index)
                .last()
        };
        if let Some(index) = last(&mut ui.scroll_timeline_name_iter()) {
            return Some(Declared::Scroll(ui.scroll_timeline_axis_mod(index)));
        }
        let index = last(&mut ui.view_timeline_name_iter())?;
        Some(Declared::View(
            ui.view_timeline_axis_mod(index),
            ui.view_timeline_inset_mod(index),
        ))
    }

    /// Whether `element`'s `timeline-scope` limits `reference` to its subtree.
    fn limits(&self, element: NodeId, style: &ComputedValues, reference: &Reference<'_>) -> bool {
        let scope = style.clone_timeline_scope();
        (scope.value.is_all() || scope.value.iter().any(|atom| atom == reference.atom))
            && self.name_tree(element, scope.scope) == reference.tree
    }

    /// The timeline `definer` declares: a named scroll timeline on a box
    /// that is not a scroll container exists and is inactive (§2.3.2).
    fn defined_timeline(&self, definer: NodeId, declared: Declared) -> Option<Timeline> {
        match declared {
            Declared::Scroll(axis) => {
                self.is_scroll_container(definer)
                    .then_some(Timeline::Scroll {
                        source: definer,
                        axis: Axis::of(axis),
                    })
            }
            Declared::View(axis, inset) => Some(Timeline::View {
                subject: definer,
                axis: Axis::of(axis),
                inset,
            }),
        }
    }

    /// The last of `scope`'s flat-tree descendants in flat tree order that
    /// defines `reference`, outside every nested `timeline-scope` limiting
    /// it.
    ///
    /// Each candidate's flat-tree path is walked once; the order is then
    /// settled level by level from `scope` down, scanning a node's children
    /// only where the surviving candidates branch, so the cost is the
    /// candidates times their depth plus the children of those branch
    /// points, never a sibling scan per pair.
    fn last_definer_under(&self, scope: NodeId, reference: &Reference<'_>) -> Option<NodeId> {
        let timelines = &self.animations().timelines;
        // Each surviving candidate's path from `scope`'s child down to it.
        let mut paths: SmallVec<[SmallVec<[NodeId; 16]>; 4]> = SmallVec::new();
        for &candidate in timelines.definers.get(reference.atom)? {
            if candidate == scope
                || !self.paint_style(candidate).is_some_and(|style| {
                    self.declared(candidate, style.get_ui(), reference)
                        .is_some()
                })
            {
                continue;
            }
            let mut path: SmallVec<[NodeId; 16]> = SmallVec::new();
            let mut current = Some(candidate);
            let under = loop {
                let Some(id) = current else {
                    break false;
                };
                if id == scope {
                    break true;
                }
                if timelines.scopers.contains(&id)
                    && self
                        .paint_style(id)
                        .is_some_and(|style| self.limits(id, style, reference))
                {
                    break false;
                }
                path.push(id);
                current = self.get(id).and_then(crate::Node::flat_parent_id);
            };
            if under {
                path.reverse();
                paths.push(path);
            }
        }
        let mut parent = scope;
        let mut depth = 0;
        loop {
            if paths.len() <= 1 {
                return paths.first().and_then(|path| path.last().copied());
            }
            // `parent`, if a candidate, precedes the rest: its descendants.
            paths.retain(|path| path.len() > depth);
            let first = paths[0][depth];
            let last = if paths.iter().all(|path| path[depth] == first) {
                first
            } else {
                let branches: FxHashSet<NodeId> = paths.iter().map(|path| path[depth]).collect();
                *self
                    .get(parent)?
                    .flat_children()
                    .iter()
                    .rev()
                    .find(|child| branches.contains(child))?
            };
            paths.retain(|path| path[depth] == last);
            parent = last;
            depth += 1;
        }
    }
}

#[cfg(test)]
mod tests;
