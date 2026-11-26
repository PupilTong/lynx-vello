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
//!   layout pass, writes its sample and places its range keyframes (`entry 0% { }`) against the
//!   timeline's named ranges. A change re-cascades the element inside the same `layout()` call —
//!   scroll-animations-1 §5.1's stale-timelines pass — so the commit that creates an animation
//!   already shows it at its offset, with its keyframes in place.
//! - [`Document::advance_scroll_timelines`] re-samples, between commits, the animations a moved
//!   scroll container drives.
//!
//! Both sample the source's offset clamped to its scroll range `[0, max]`, as the painter's
//! exported curves do. On an `overscroll-behavior: circular` axis the document's offset stands
//! anywhere in the period `[0, P)`; a scroll progress timeline reads an offset past `max` at its
//! representative nearer the range ([`ProgressKind`]), a view progress timeline reads it as it
//! stands, unclamped.
//!
//! Where the specifications are silent or disagree this follows Blink;
//! `docs/tracking/css-animation.md` lists each choice.

use euclid::default::{Size2D, Vector2D};
use rustc_hash::{FxHashMap, FxHashSet};
use seldon::style::PositionProperty;
use smallvec::SmallVec;
use stylo::dom::OpaqueNode;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::UI;
use stylo::rule_tree::CascadeLevel;
use stylo::servo::animation::{
    Animation, AnimationProgress, AnimationSetKey, AnimationState, TimelineRanges,
};
use stylo::values::computed::{
    AnimationDirection, AnimationFillMode, AnimationTimeline, Length, LengthPercentage, ScrollAxis,
    TimelineName, ViewTimelineInset,
};
use stylo::values::generics::animation::AnimationRangeValue;
use stylo::values::generics::length::LengthPercentageOrAuto;
use stylo::values::specified::animation::{Scroller, TimelineRangeName};
use stylo_atoms::Atom;

use crate::layout::{DisplayMode, display_mode};
use crate::scroll::ScrollBox;
use crate::scroll::snap::scroll_padding;
use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeId};
use crate::visual::sticky::{self, StickyAxis};

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

    pub(crate) const fn of_vector(self, vector: Vector2D<f32>) -> f32 {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProgressTiming {
    pub start: f32,
    pub origin: f64,
    pub end: f32,
    pub iteration: f64,
    pub limit: f32,
    pub iterations: f64,
    pub direction: AnimationDirection,
    pub fill: AnimationFillMode,
}

impl ProgressTiming {
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
        let proportions = match duration {
            _ if !counted => None,
            None => Some((0.0, length / iterations)),
            Some(duration) => {
                let total = delay + duration * iterations;
                (total > 0.0).then_some((delay * length / total, duration * length / total))
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

    fn active(&self) -> f64 {
        (f64::from(self.end) - self.origin).max(0.0)
    }

    pub(crate) fn contributes_throughout(&self) -> bool {
        let backwards = matches!(
            self.fill,
            AnimationFillMode::Backwards | AnimationFillMode::Both
        );
        let forwards = matches!(
            self.fill,
            AnimationFillMode::Forwards | AnimationFillMode::Both
        );
        let before = 0.0 < self.origin.max(f64::from(self.start));
        let after = self.limit > self.end;
        (backwards || !before) && (forwards || !after)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Before,
    Active,
    After,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProgressKind {
    Scroll,
    View,
}

impl ProgressKind {
    pub(crate) fn offset_of(self, scroll_box: &ScrollBox) -> Vector2D<f32> {
        match self {
            Self::Scroll => scroll_box.unwrapped_offset(),
            Self::View => scroll_box.offset,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Binding {
    Active {
        source: NodeId,
        axis: Axis,
        timing: ProgressTiming,
        kind: ProgressKind,
    },
    Inactive,
}

impl Binding {
    const fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    fn sample(
        &self,
        scroll_box_of: impl Fn(NodeId) -> Option<ScrollBox>,
    ) -> Option<AnimationProgress> {
        match self {
            Self::Active {
                source,
                axis,
                timing,
                kind,
            } => {
                let scroll_box = scroll_box_of(*source);
                let offset = scroll_box
                    .as_ref()
                    .map_or_else(Vector2D::zero, |scroll_box| kind.offset_of(scroll_box));
                let offset = axis.of_vector(offset);
                let wraps = *kind == ProgressKind::View
                    && scroll_box.is_some_and(|scroll_box| {
                        let (x, y) = scroll_box.wrap_period();
                        match axis {
                            Axis::X => x.is_some(),
                            Axis::Y => y.is_some(),
                        }
                    });
                let offset = if wraps {
                    offset
                } else {
                    offset.max(0.0).min(timing.limit)
                };
                iteration_progress(timing, offset)
            }
            Self::Inactive => None,
        }
    }
}

#[derive(Debug)]
struct Bound {
    binding: Binding,
    sampled: bool,
}

#[derive(Default)]
pub(crate) struct ScrollTimelines {
    pub(crate) progress_driven: FxHashSet<NodeId>,
    bindings: FxHashMap<(NodeId, Atom), Bound>,
    spare: FxHashMap<(NodeId, Atom), Bound>,
    dependents: FxHashMap<NodeId, SmallVec<[(NodeId, Atom); 2]>>,
    definers: FxHashMap<Atom, SmallVec<[NodeId; 2]>>,
    defined: FxHashMap<NodeId, SmallVec<[Atom; 2]>>,
    scopers: FxHashSet<NodeId>,
    pub(crate) painted: FxHashSet<NodeId>,
}

impl ScrollTimelines {
    pub(crate) fn binding_of(&self, id: NodeId, name: &Atom) -> Option<&Binding> {
        self.bindings
            .get(&(id, name.clone()))
            .map(|bound| &bound.binding)
    }

    #[cfg(test)]
    pub(crate) fn binding(&self, id: NodeId, name: &str) -> Option<&Binding> {
        self.binding_of(id, &Atom::from(name))
    }

    #[cfg(test)]
    pub(crate) fn defines(&self, id: NodeId) -> bool {
        self.defined.contains_key(&id) || self.scopers.contains(&id)
    }

    pub(crate) fn has_definers(&self) -> bool {
        !self.defined.is_empty() || !self.scopers.is_empty()
    }

    pub(crate) fn is_current(&self, id: NodeId, name: &Atom) -> bool {
        self.bindings
            .get(&(id, name.clone()))
            .is_none_or(|bound| bound.binding.is_active())
    }

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
            self.painted.remove(&id);
        }
        self.bindings.retain(|(id, _), _| !ids.contains(id));
        for list in self.dependents.values_mut() {
            list.retain(|(id, _)| !ids.contains(id));
        }
    }
}

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

enum Declared {
    Scroll(ScrollAxis),
    View(ScrollAxis, ViewTimelineInset),
}

struct Reference<'a> {
    atom: &'a Atom,
    tree: Option<NodeId>,
}

struct NamedRanges {
    cover: [f32; 2],
    contain: [f32; 2],
    entry: [f32; 2],
    exit: [f32; 2],
    entry_crossing: [f32; 2],
    exit_crossing: [f32; 2],
    scroll: [f32; 2],
}

impl NamedRanges {
    const fn scroll(max: f32) -> Self {
        let range = [0.0, max];
        Self {
            cover: range,
            contain: range,
            entry: range,
            exit: range,
            entry_crossing: range,
            exit_crossing: range,
            scroll: range,
        }
    }

    const fn named(&self, name: TimelineRangeName) -> Option<[f32; 2]> {
        Some(match name {
            TimelineRangeName::Normal | TimelineRangeName::None => return None,
            TimelineRangeName::Cover => self.cover,
            TimelineRangeName::Contain => self.contain,
            TimelineRangeName::Entry => self.entry,
            TimelineRangeName::Exit => self.exit,
            TimelineRangeName::EntryCrossing => self.entry_crossing,
            TimelineRangeName::ExitCrossing => self.exit_crossing,
            TimelineRangeName::Scroll => self.scroll,
        })
    }
}

fn keyframe_ranges(attachment: [f32; 2], ranges: &NamedRanges) -> Option<TimelineRanges> {
    let [start, end] = attachment.map(f64::from);
    let length = end - start;
    if length <= 0.0 {
        return None;
    }
    TimelineRanges::from_fn(|name| {
        Some(
            ranges
                .named(name)?
                .map(|bound| (f64::from(bound) - start) / length),
        )
    })
}

fn range_point(
    value: &AnimationRangeValue<LengthPercentage>,
    end: bool,
    ranges: &NamedRanges,
) -> f32 {
    if value.name == TimelineRangeName::Normal {
        return ranges.cover[usize::from(end)];
    }
    let [from, to] = ranges.named(value.name).unwrap_or(ranges.cover);
    from + value.lp.resolve(Length::new(to - from)).px()
}

fn attachment(ui: &UI, index: usize, ranges: &NamedRanges) -> [f32; 2] {
    [
        range_point(&ui.animation_range_start_mod(index).0, false, ranges),
        range_point(&ui.animation_range_end_mod(index).0, true, ranges),
    ]
}

fn timing(ui: &UI, index: usize, attachment: [f32; 2], limit: f32) -> ProgressTiming {
    let duration = ui.animation_duration_mod(index);
    ProgressTiming::normalized(
        limit,
        attachment,
        (!duration.is_auto()).then(|| f64::from(duration.seconds())),
        f64::from(ui.animation_delay_mod(index).seconds()),
        f64::from(ui.animation_iteration_count_mod(index).0),
        ui.animation_direction_mod(index),
        ui.animation_fill_mode_mod(index),
    )
}

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
                    let (binding, ranges) = document.bind(id, animation);
                    let key = (id, animation.name.clone());
                    let old = previous.remove(&key);
                    let mut sampled = old.as_ref().is_some_and(|bound| bound.sampled);
                    let was_active = old.as_ref().is_none_or(|bound| bound.binding.is_active());
                    flipped |= was_active != binding.is_active();
                    changed |= old.is_none_or(|bound| bound.binding != binding);
                    changed |= !skipped && animation.set_timeline_ranges(ranges);
                    let paused = matches!(animation.state, AnimationState::Paused(_));
                    if !skipped && (!paused || !sampled) {
                        sampled = true;
                        let sample = binding.sample(|source| document.scroll_box(source));
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
        relayout | self.sync_animation_state()
    }

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
                    if timelines.painted.contains(&id) {
                        continue;
                    }
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
                    let sample = bound.binding.sample(|source| self.scroll_box(source));
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

    fn bind(&self, id: NodeId, animation: &Animation) -> (Binding, Option<TimelineRanges>) {
        let Some(ui) = self.paint_style(id).map(|style| style.get_ui()) else {
            return (Binding::Inactive, None);
        };
        let Some(index) = ui
            .animation_name_iter()
            .position(|name| name.as_atom() == Some(&animation.name))
        else {
            return (Binding::Inactive, None);
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
            .unwrap_or((Binding::Inactive, None))
    }

    fn root_scroller(&self) -> Option<NodeId> {
        Some(DOCUMENT_ELEMENT_NODE_ID).filter(|&root| self.is_scroll_container(root))
    }

    fn bind_to(
        &self,
        timeline: &Timeline,
        ui: &UI,
        index: usize,
    ) -> Option<(Binding, Option<TimelineRanges>)> {
        let (source, axis, ranges, kind) = match timeline {
            &Timeline::Scroll { source, axis } => {
                let max = axis.of_vector(self.scroll_box(source)?.max_offset());
                (source, axis, NamedRanges::scroll(max), ProgressKind::Scroll)
            }
            Timeline::View {
                subject,
                axis,
                inset,
            } => {
                let source = self.nearest_scroll_container(*subject)?;
                let ranges = self.view_ranges(*subject, source, *axis, inset)?;
                (source, *axis, ranges, ProgressKind::View)
            }
        };
        (ranges.cover[1] > ranges.cover[0]).then(|| {
            let attachment = attachment(ui, index, &ranges);
            let binding = Binding::Active {
                source,
                axis,
                timing: timing(ui, index, attachment, ranges.scroll[1]),
                kind,
            };
            (binding, keyframe_ranges(attachment, &ranges))
        })
    }

    fn view_ranges(
        &self,
        subject: NodeId,
        source: NodeId,
        axis: Axis,
        inset: &ViewTimelineInset,
    ) -> Option<NamedRanges> {
        let node = self.get(subject)?;
        let style = node.layout_computed_style()?;
        if matches!(
            display_mode(node, *style.get_display()),
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
                padding.get_scroll_padding_left(),
                padding.get_scroll_padding_right(),
            ),
            Axis::Y => (
                padding.get_scroll_padding_top(),
                padding.get_scroll_padding_bottom(),
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
        let a = shift.preimage(position - (port - inset_end));
        let b = shift.preimage(position + length - (port - inset_end));
        let c = shift.preimage(position - inset_start);
        let d = shift.preimage(position + length - inset_start);
        let contain = [b.0.min(c.0), b.1.max(c.1)];
        Some(NamedRanges {
            cover: [a.1, d.0],
            contain,
            entry: [a.1, contain[0]],
            exit: [contain[1], d.0],
            entry_crossing: [a.1, b.0],
            exit_crossing: [c.1, d.0],
            scroll: [0.0, max],
        })
    }

    fn sticky_shift(&self, subject: NodeId, source: NodeId, axis: Axis) -> Knots {
        let mut chain: SmallVec<[NodeId; 2]> = SmallVec::new();
        let mut current = Some(subject);
        while let Some(id) = current
            && id != source
        {
            if self
                .paint_style(id)
                .is_some_and(|style| *style.get_box().get_position() == PositionProperty::Sticky)
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

    fn name_tree(&self, element: NodeId, scope: CascadeLevel) -> Option<NodeId> {
        self.get(element)?.scoped_name_tree(scope)
    }

    fn names(&self, element: NodeId, name: &TimelineName, reference: &Reference<'_>) -> bool {
        name.value.as_atom() == Some(reference.atom)
            && self.name_tree(element, name.scope) == reference.tree
    }

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

    fn limits(&self, element: NodeId, style: &ComputedValues, reference: &Reference<'_>) -> bool {
        let scope = style.get_timeline_scope();
        (scope.value.is_all() || scope.value.iter().any(|atom| atom == reference.atom))
            && self.name_tree(element, scope.scope) == reference.tree
    }

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

    fn last_definer_under(&self, scope: NodeId, reference: &Reference<'_>) -> Option<NodeId> {
        let timelines = &self.animations().timelines;
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
                current = self
                    .get(id)
                    .and_then(crate::tree::node::Node::flat_parent_id);
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
