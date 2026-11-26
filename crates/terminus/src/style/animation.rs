//! The CSS animation and transition driver.
//!
//! Stylo already owns everything about an animation except its timeline:
//! `@keyframes` resolution, per-property interpolation, and the
//! `Animations`/`Transitions` cascade origins all live in
//! [`stylo::servo::animation`], and a normal style flush starts and cancels
//! animations for free through `MatchMethods::process_animations`. What Stylo
//! does not do is keep the resulting [`DocumentAnimationSet`] across flushes,
//! advance its state machine, or re-cascade the elements it moved. That is
//! this module.
//!
//! The tick needs nothing but `&mut Document`. In the Bobcat runtime the
//! document's owner thread runs it — the Lynx main thread once the script
//! starts, on the per-frame frame post that carries the presenting
//! side's clock reading — so advancing an animation costs no script and no
//! DOM mutation; starting and stopping an animation rides the style flush
//! the same thread already runs. Elements whose animated
//! properties do not affect geometry never reach layout either, because the
//! harvest only calls `invalidate_layout` for damage that
//! [`StyleDamage::needs_relayout`] reports.
//!
//! This crate owns no clock (see [`crate::input`]): `now` is a parameter. The
//! timeline therefore only moves in [`Document::advance_animations`] and
//! [`Document::sync_animation_clock`], and the presenting side is free to
//! stop ticking — an idle page sends no frame, and an animation an exported
//! curve already covers is sampled on the painting side instead. The Bobcat
//! runtime syncs the clock to the painter's at the start of every job, so a
//! flush reads the painter's latest instant; a host that does not reads
//! whatever time the last tick left behind, which can be arbitrarily far in
//! the past.
//!
//! So a created animation is not anchored to the timeline by its flush. Web
//! Animations resolves a pending animation's start time at the first frame
//! after it was created, and that is what the driver does: the first tick to
//! see a `Pending` animation or transition moves its `started_at`/`start_time`
//! forward by the interval that tick advanced the timeline over, which lands it
//! at that frame's reading. Both fields already carry the delay, so the shift
//! preserves a positive delay as well as the head start of a negative one, and
//! it also lands the start time Stylo computes when a paused animation resumes.
//! An animation shifted once is recorded as anchored and never shifted again,
//! however many frames its delay keeps it `Pending` for.
//!
//! # Side effects of animation
//!
//! While an `opacity` or `transform` animation or transition is current or
//! in effect, web-animations-1 has its element behave as if `will-change`
//! named the property: a stacking context either way, a group and Backdrop
//! Root for `opacity`, the absolute and fixed containing block for
//! `transform`. [`Document::sync_animation_state`] records that as the node's
//! `animates` bits, so the paint build and layout read a bit, never the map,
//! and none of it depends on whether a curve exports. A transform bit that
//! flips relayouts the element's positioned descendants.
//!
//! # Scroll-driven animations
//!
//! An animation on a scroll or view progress timeline takes no clock at all:
//! stylo never iterates, ends or ticks it, and cascades the progress
//! [`crate::style::timeline`] writes from its scroll container's offset. The
//! driver only records which elements hold one, for that module to bind.
//!
//! # Frozen animations: css-contain-2 §4
//!
//! > While an element is skipped, CSS transitions and animations on the
//! > element do not update: *"New animations are not created even if
//! > newly-applied style would start one. Existing animations do not advance
//! > in their timeline. Running animations on the element do not end."*
//! >
//! > When an element stops being skipped, animations and transitions are
//! > sampled and then resume advancing on their timelines as normal from that
//! > point.
//! >
//! > — [css-contain-2 §4](https://drafts.csswg.org/css-contain-2/#content-visibility)
//!
//! An element "is skipped" when it is part of some ancestor's *skipped
//! contents* — "the flat tree descendants of the element", so the box that
//! skips is itself **not** skipped and its own animations run as normal. That
//! is [`Document::in_skipped_subtree`], one flat-ancestor walk per element
//! that owns animation state, asking `crate::layout::skips_contents` — the
//! same predicate layout and the paint build ask, so `content-visibility:
//! hidden` and a non-relevant `content-visibility: auto` box freeze by the
//! same rule they skip by.
//!
//! Freezing is the anchoring arithmetic above, applied to an animation that
//! is already running: a tick that finds an element frozen moves every one of
//! its start times forward by the interval it advanced the timeline over, so
//! `now - started_at` — the animation's progress — does not move, and nothing
//! is promoted, iterated, ended or re-cascaded. Nothing samples a frozen
//! element either: [`Document::has_active_animations`] and the committed
//! frame's animation flags ignore frozen sets, so a page whose only
//! animations are frozen leaves the painter's `owes_frame` false and the host
//! stops ticking entirely.
//!
//! Which is why the carry is decided by [`AnimationDriver::carried`] — the
//! elements frozen *when the last tick ended* — as well as by the freeze as
//! of this tick. A reveal is noticed between two ticks, by a style flush or
//! by the relevance pass inside `Document::render`, and the engine has no
//! reading for the instant it happened; the interval containing it may even
//! be an arbitrarily long stretch the host never ticked at all, precisely
//! because a frozen page owes no frames. Carrying that whole interval is the
//! only rule that survives it, so an animation resumes at **the first tick
//! after the reveal** rather than at the reveal itself.
//!
//! One deviation from the list above: *new animations are not created*. This
//! engine cannot honor that, because skipping contents does not skip style —
//! Stylo's `process_animations` runs in the ordinary flush and creates the
//! animation or transition the new style names, whatever box is above it. So
//! a keyframe animation or a transition that starts inside a skipped subtree
//! is created and then immediately frozen at its own start, and plays from
//! zero when the subtree reveals. Recorded in `docs/style-assumptions.md`
//! §19.

use rustc_hash::FxHashSet;
use stylo::context::{SharedStyleContext, StyleSystemOptions};
use stylo::dom::OpaqueNode;
use stylo::driver;
use stylo::invalidation::element::restyle_hints::RestyleHint;
use stylo::properties::longhands::animation_fill_mode::computed_value::single_value::T as AnimationFillMode;
use stylo::properties::{LonghandId, OwnedPropertyDeclarationId, PropertyDeclarationId};
use stylo::selector_parser::SnapshotMap;
use stylo::servo::animation::{
    Animation, AnimationSetKey, AnimationState, DocumentAnimationSet, ElementAnimationSet,
    Transition,
};
use stylo::traversal_flags::TraversalFlags;
use stylo_atoms::Atom;

use crate::layout::skips_contents;
use crate::style::damage::StyleDamage;
use crate::style::flush::{LayoutThreadStateGuard, NO_PAINTERS, RecalcStyle};
use crate::style::timeline::ScrollTimelines;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::{ANIMATES_OPACITY, ANIMATES_TRANSFORM, Node};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimationTick {
    pub needs_next_frame: bool,
    pub restyled: usize,
    pub relayout: bool,
}

fn is_cancelled(animation: &impl Cancellable) -> bool {
    animation.is_cancelled()
}

trait Cancellable {
    fn is_cancelled(&self) -> bool;
}

impl Cancellable for Animation {
    fn is_cancelled(&self) -> bool {
        self.state == AnimationState::Canceled
    }
}

impl Cancellable for Transition {
    fn is_cancelled(&self) -> bool {
        self.state == AnimationState::Canceled
    }
}

const COMPOSITES: [(LonghandId, u8); 5] = [
    (LonghandId::Opacity, ANIMATES_OPACITY),
    (LonghandId::Transform, ANIMATES_TRANSFORM),
    (LonghandId::Translate, ANIMATES_TRANSFORM),
    (LonghandId::Rotate, ANIMATES_TRANSFORM),
    (LonghandId::Scale, ANIMATES_TRANSFORM),
];

pub(crate) fn animation_has_side_effects(
    animation: &Animation,
    timeline_active: impl FnOnce() -> bool,
) -> bool {
    if animation.is_progress_driven() {
        return animation.state != AnimationState::Canceled && timeline_active();
    }
    match animation.state {
        AnimationState::Pending | AnimationState::Running | AnimationState::Paused(_) => true,
        AnimationState::Finished => matches!(
            animation.fill_mode,
            AnimationFillMode::Forwards | AnimationFillMode::Both
        ),
        AnimationState::Canceled => false,
    }
}

fn transition_has_side_effects(transition: &Transition) -> bool {
    matches!(
        transition.state,
        AnimationState::Pending | AnimationState::Running
    )
}

fn composite_bit(property: PropertyDeclarationId<'_>) -> u8 {
    COMPOSITES
        .iter()
        .find(|(longhand, _)| property.as_longhand() == Some(*longhand))
        .map_or(0, |(_, bit)| *bit)
}

#[derive(Default)]
struct Stepped {
    hinted: Vec<NodeId>,
    moved: bool,
    finished: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Tick,
    Sync,
}

#[derive(PartialEq, Eq, Hash)]
struct AnchoredAnimation {
    set: AnimationSetKey,
    what: AnchoredKind,
}

#[derive(PartialEq, Eq, Hash)]
enum AnchoredKind {
    Keyframes(Atom),
    Transition(OwnedPropertyDeclarationId),
}

struct Step<'a> {
    now: f64,
    shift: f64,
    skipped: bool,
    carry: bool,
    pass: Pass,
    finished: bool,
    anchored: &'a FxHashSet<AnchoredAnimation>,
    pending: &'a mut FxHashSet<AnchoredAnimation>,
}

impl Step<'_> {
    const fn shifts_pending(&self, fresh: bool) -> bool {
        self.carry || fresh
    }

    const fn settles(&self, fresh: bool) -> bool {
        !self.skipped && (fresh || self.carry)
    }

    const fn defers(&self, fresh: bool) -> bool {
        fresh && matches!(self.pass, Pass::Sync)
    }

    fn animations(
        &mut self,
        id: NodeId,
        key: &AnimationSetKey,
        animations: &mut [Animation],
        held: &mut Vec<(NodeId, Animation)>,
    ) -> bool {
        let mut moved = false;
        for animation in animations {
            if animation.state == AnimationState::Pending {
                let entry = AnchoredAnimation {
                    set: key.clone(),
                    what: AnchoredKind::Keyframes(animation.name.clone()),
                };
                let fresh = !self.anchored.contains(&entry);
                if self.shifts_pending(fresh) {
                    animation.started_at += self.shift;
                }
                if self.defers(fresh) {
                    continue;
                }
                if !self.skipped && animation.started_at <= self.now {
                    animation.state = AnimationState::Running;
                    moved = true;
                } else {
                    moved |= self.settles(fresh);
                    self.pending.insert(entry);
                }
            } else if self.carry {
                animation.started_at += self.shift;
                moved |= self.settles(false);
            }
            if self.skipped {
                continue;
            }
            if animation.iterate_to(self.now) {
                moved = true;
            }
            if animation.state == AnimationState::Running && animation.has_ended(self.now) {
                animation.state = AnimationState::Finished;
                moved = true;
                self.finished = true;
                if matches!(
                    animation.fill_mode,
                    AnimationFillMode::Forwards | AnimationFillMode::Both
                ) {
                    held.push((id, animation.clone()));
                }
            }
        }
        moved
    }

    fn transitions(&mut self, key: &AnimationSetKey, transitions: &mut [Transition]) -> bool {
        let mut moved = false;
        for transition in transitions {
            if transition.state == AnimationState::Pending {
                let entry = AnchoredAnimation {
                    set: key.clone(),
                    what: AnchoredKind::Transition(
                        transition.property_animation.property_id().to_owned(),
                    ),
                };
                let fresh = !self.anchored.contains(&entry);
                if self.shifts_pending(fresh) {
                    transition.start_time += self.shift;
                }
                if self.defers(fresh) {
                    continue;
                }
                if !self.skipped && transition.start_time <= self.now {
                    transition.state = AnimationState::Running;
                    moved = true;
                } else {
                    moved |= self.settles(fresh);
                    self.pending.insert(entry);
                }
            } else if self.carry {
                transition.start_time += self.shift;
                moved |= self.settles(false);
            }
            if self.skipped {
                continue;
            }
            if transition.state == AnimationState::Running && transition.has_ended(self.now) {
                transition.state = AnimationState::Finished;
                moved = true;
                self.finished = true;
            }
        }
        moved
    }
}

#[derive(Default)]
pub(crate) struct AnimationDriver {
    sets: DocumentAnimationSet,
    now: f64,
    active: bool,
    frozen: bool,
    carried: FxHashSet<NodeId>,
    carried_spare: FxHashSet<NodeId>,
    marked: Vec<NodeId>,
    flagged: Vec<NodeId>,
    anchored: FxHashSet<AnchoredAnimation>,
    anchored_spare: FxHashSet<AnchoredAnimation>,
    held: Vec<(NodeId, Animation)>,
    owed: FxHashSet<NodeId>,
    pub(crate) timelines: ScrollTimelines,
}

impl AnimationDriver {
    pub(crate) fn context_handle(&self) -> DocumentAnimationSet {
        self.sets.clone()
    }

    pub(crate) fn keyframes_anchored(&self, set: &AnimationSetKey, name: &Atom) -> bool {
        self.anchored.contains(&AnchoredAnimation {
            set: set.clone(),
            what: AnchoredKind::Keyframes(name.clone()),
        })
    }

    pub(crate) fn transition_anchored(
        &self,
        set: &AnimationSetKey,
        property: PropertyDeclarationId<'_>,
    ) -> bool {
        self.anchored.contains(&AnchoredAnimation {
            set: set.clone(),
            what: AnchoredKind::Transition(property.to_owned()),
        })
    }

    pub(crate) fn carries(&self, id: NodeId) -> bool {
        self.carried.contains(&id)
    }

    pub(crate) const fn now(&self) -> f64 {
        self.now
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn has_frozen_animations(&self) -> bool {
        self.frozen || !self.carried.is_empty()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sets.sets.read().is_empty() && !self.timelines.has_definers()
    }

    pub(crate) fn forget(&mut self, ids: &[NodeId]) {
        if ids.is_empty() {
            return;
        }
        self.flagged.retain(|flagged| !ids.contains(flagged));
        self.marked.retain(|marked| !ids.contains(marked));
        self.carried.retain(|carried| !ids.contains(carried));
        self.held.retain(|(id, _)| !ids.contains(id));
        self.owed.retain(|owed| !ids.contains(owed));
        self.timelines.forget(ids);
        self.anchored
            .retain(|entry| !ids.iter().any(|id| id.arena_key() == entry.set.node.0));
        let mut sets = self.sets.sets.write();
        if sets.is_empty() {
            return;
        }
        let before = sets.len();
        sets.retain(|entry, _| !ids.iter().any(|id| id.arena_key() == entry.node.0));
        if sets.len() == before {
            return;
        }
        self.active = sets
            .values()
            .any(stylo::servo::animation::ElementAnimationSet::needs_animation_ticks);
    }
}

impl<T: Sync> Document<T> {
    #[must_use]
    pub fn animation_clock(&self) -> f64 {
        self.animations().now()
    }

    #[must_use]
    pub fn has_active_animations(&self) -> bool {
        self.animations().is_active() || self.animated_images_run()
    }

    pub(crate) fn in_skipped_subtree(&self, id: NodeId) -> bool {
        let mut current = self.get(id).and_then(Node::flat_parent_id);
        while let Some(ancestor) = current {
            let Some(node) = self.get(ancestor) else {
                break;
            };
            if node
                .layout_computed_style()
                .is_some_and(|style| skips_contents(node, style))
            {
                return true;
            }
            current = node.flat_parent_id();
        }
        false
    }

    pub(crate) fn refresh_animation_activity(
        &mut self,
        frame: Option<&crate::visual::PaintOrder>,
    ) -> (bool, bool) {
        let handle = self.animations().context_handle();
        let (active, frozen, needs_main_ticks) = {
            let sets = handle.sets.read();
            let mut frozen = false;
            let mut ticking = 0usize;
            for (key, set) in &*sets {
                let Some(id) = self.arenas().id_at_arena_key(key.node.0) else {
                    continue;
                };
                if self.in_skipped_subtree(id) {
                    frozen = true;
                } else if set.needs_animation_ticks() {
                    ticking += 1;
                }
            }
            let covered = frame.map_or(0, |frame| {
                frame
                    .animations()
                    .iter()
                    .filter(|slot| {
                        debug_assert!(!self.in_skipped_subtree(slot.node));
                        let key =
                            AnimationSetKey::new_for_non_pseudo(OpaqueNode(slot.node.arena_key()));
                        sets.get(&key)
                            .is_some_and(ElementAnimationSet::needs_animation_ticks)
                    })
                    .count()
            });
            (ticking > 0, frozen, frame.is_some() && ticking > covered)
        };
        let driver = self.animations_mut();
        driver.active = active;
        driver.frozen = frozen;
        (active, needs_main_ticks)
    }

    pub fn advance_animations(&mut self, now: f64) -> AnimationTick {
        let mut tick = self.step_timeline(now, Pass::Tick);
        if !self.animated_images.is_empty() {
            self.advance_animated_images(now, &mut tick);
        }
        tick
    }

    pub fn sync_animation_clock(&mut self, now: f64) {
        let _ = self.step_timeline(now, Pass::Sync);
    }

    fn step_timeline(&mut self, now: f64, pass: Pass) -> AnimationTick {
        let was_active = self.animations().is_active();
        let owed = pass == Pass::Tick && !self.animations().owed.is_empty();
        if !was_active && !self.animations().has_frozen_animations() && !owed {
            let driver = self.animations_mut();
            driver.now = driver.now.max(now);
            return AnimationTick::default();
        }
        let previous = self.animations().now;
        let now = now.max(previous);
        if pass == Pass::Sync && now <= previous {
            return AnimationTick::default();
        }
        self.animations_mut().now = now;

        let Stepped {
            mut hinted,
            mut moved,
            finished,
        } = self.step_animation_states(now, now - previous, pass);
        if pass == Pass::Sync && !finished {
            self.animations_mut().owed.extend(hinted);
            return AnimationTick::default();
        }
        let owed = std::mem::take(&mut self.animations_mut().owed);
        moved |= !owed.is_empty();
        hinted.extend(owed);
        if hinted.is_empty() {
            let relayout = self.sync_animation_state();
            if moved || (was_active && !self.animations().is_active()) {
                self.note_visual_mutation();
            }
            return AnimationTick {
                relayout,
                ..AnimationTick::default()
            };
        }

        let root = self.hint_animated_elements(&hinted);
        let mut tick = self.recascade_animated_elements(root);
        tick.relayout |= self.sync_animation_state();
        tick.needs_next_frame = self.animations().is_active();
        if tick.restyled > 0 || moved || (was_active && !tick.needs_next_frame) {
            self.note_visual_mutation();
        }
        tick
    }

    pub(crate) fn sync_animation_state(&mut self) -> bool {
        let cancelled = self.drop_cancelled_animations();
        let mut relayout = !cancelled.is_empty() && self.recascade_cancelled_animations(&cancelled);

        let handle = self.animations().context_handle();
        let mut held = std::mem::take(&mut self.animations_mut().held);
        let mut animated = Vec::new();
        let mut progress_driven =
            std::mem::take(&mut self.animations_mut().timelines.progress_driven);
        progress_driven.clear();
        let mut flipped = Vec::new();
        {
            let mut sets = handle.sets.write();
            if !held.is_empty() {
                self.restore_held_animations(&mut held, &mut sets);
            }
            for (key, set) in &*sets {
                let Some(id) = self.arenas().id_at_arena_key(key.node.0) else {
                    continue;
                };
                if key.pseudo_element.is_none()
                    && let Some(node) = self.get(id)
                {
                    if set.animations.iter().any(|animation| {
                        animation.is_progress_driven()
                            && animation.state != AnimationState::Canceled
                    }) {
                        progress_driven.insert(id);
                    }
                    let bits = self.animates(node, set);
                    let changed = node.replace_animates(bits) ^ bits;
                    if changed != 0 {
                        flipped.push((id, changed));
                    }
                }
                if !set.is_empty() {
                    animated.push(id);
                }
            }
            sets.retain(|_, set| !set.is_empty());
            for &id in &self.animations().flagged {
                let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(id.arena_key()));
                if !sets.contains_key(&key)
                    && let Some(node) = self.get(id)
                {
                    let changed = node.replace_animates(0);
                    if changed != 0 {
                        flipped.push((id, changed));
                    }
                }
            }
        }
        self.animations_mut().held = held;
        self.animations_mut().timelines.progress_driven = progress_driven;
        if !flipped.is_empty() {
            self.note_visual_mutation();
        }
        for (id, changed) in flipped {
            if changed & ANIMATES_TRANSFORM != 0 {
                relayout |= self.invalidate_containing_block(id);
            }
        }

        let mut flagged = std::mem::take(&mut self.animations_mut().flagged);
        for &id in &flagged {
            if let Some(node) = self.get(id) {
                node.set_may_have_animations(false);
            }
        }
        flagged.clear();
        for id in animated {
            if let Some(node) = self.get(id) {
                node.set_may_have_animations(true);
                flagged.push(id);
            }
        }
        self.animations_mut().flagged = flagged;
        self.refresh_animation_activity(None);
        relayout
    }

    fn drop_cancelled_animations(&mut self) -> Vec<NodeId> {
        let handle = self.animations().context_handle();
        let mut held = std::mem::take(&mut self.animations_mut().held);
        let mut cancelled = Vec::new();
        {
            let mut sets = handle.sets.write();
            if !held.is_empty() {
                self.restore_held_animations(&mut held, &mut sets);
            }
            for (key, set) in &mut *sets {
                let Some(id) = self.arenas().id_at_arena_key(key.node.0) else {
                    continue;
                };
                if set.animations.iter().any(is_cancelled)
                    || set.transitions.iter().any(is_cancelled)
                {
                    cancelled.push(id);
                    set.clear_canceled_animations();
                }
            }
        }
        self.animations_mut().held = held;
        cancelled
    }

    fn animates(&self, node: &Node<T>, set: &ElementAnimationSet) -> u8 {
        let timelines = &self.animations().timelines;
        let animations = set
            .animations
            .iter()
            .filter(|animation| {
                animation_has_side_effects(animation, || {
                    timelines.is_current(node.id(), &animation.name)
                })
            })
            .flat_map(Animation::animating_properties)
            .fold(0, |bits, (_, property)| bits | composite_bit(property));
        set.transitions
            .iter()
            .filter(|transition| transition_has_side_effects(transition))
            .fold(animations, |bits, transition| {
                bits | composite_bit(transition.property_animation.property_id())
            })
    }

    fn recascade_cancelled_animations(&mut self, cancelled: &[NodeId]) -> bool {
        let root = self.hint_animated_elements(cancelled);
        let tick = self.recascade_animated_elements(root);
        if tick.restyled > 0 {
            self.note_visual_mutation();
        }
        tick.relayout
    }

    fn restore_held_animations(
        &self,
        held: &mut Vec<(NodeId, Animation)>,
        sets: &mut stylo::FxHashMap<AnimationSetKey, ElementAnimationSet>,
    ) {
        held.retain(|(id, animation)| {
            if self.arenas().get(*id).is_none() {
                return false;
            }
            let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(id.arena_key()));
            sets.get(&key).is_none_or(|set| {
                !set.animations.iter().any(|live| {
                    live.name == animation.name && live.state == AnimationState::Canceled
                })
            })
        });
        for (id, animation) in held.iter() {
            let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(id.arena_key()));
            let set = sets.entry(key).or_default();
            if !set
                .animations
                .iter()
                .any(|live| live.name == animation.name)
            {
                set.animations.push(animation.clone());
            }
        }
    }

    fn step_animation_states(&mut self, now: f64, shift: f64, pass: Pass) -> Stepped {
        let handle = self.animations().context_handle();
        let mut anchored = std::mem::take(&mut self.animations_mut().anchored);
        let mut pending = std::mem::take(&mut self.animations_mut().anchored_spare);
        pending.clear();
        let mut was_frozen = std::mem::take(&mut self.animations_mut().carried);
        let mut frozen = std::mem::take(&mut self.animations_mut().carried_spare);
        frozen.clear();
        let document: &Self = self;
        let arenas = document.arenas();
        let mut stepped = Stepped::default();
        let mut held = Vec::new();

        let mut sets = handle.sets.write();
        sets.retain(|key, set| {
            let Some(id) = arenas.id_at_arena_key(key.node.0) else {
                return false;
            };
            let skipped = document.in_skipped_subtree(id);
            let mut step = Step {
                now,
                shift,
                skipped,
                carry: skipped || was_frozen.contains(&id),
                pass,
                finished: false,
                anchored: &anchored,
                pending: &mut pending,
            };
            if skipped {
                frozen.insert(id);
            }
            let mut moved = step.animations(id, key, &mut set.animations, &mut held);
            moved |= step.transitions(key, &mut set.transitions);
            stepped.moved |= moved;
            stepped.finished |= step.finished;
            set.clear_canceled_animations();
            if set.is_empty() {
                return false;
            }
            let ticks = pass == Pass::Tick && set.needs_animation_ticks();
            if !skipped && (ticks || moved) {
                stepped.hinted.push(id);
            }
            true
        });
        drop(sets);
        anchored.clear();
        was_frozen.clear();
        let driver = self.animations_mut();
        driver.anchored = pending;
        driver.anchored_spare = anchored;
        driver.carried = frozen;
        driver.carried_spare = was_frozen;
        driver.held.append(&mut held);
        stepped
    }

    pub(super) fn hint_animated_elements(&mut self, hinted: &[NodeId]) -> NodeId {
        for &id in hinted {
            let Some(node) = self.arenas_mut().get_mut(id) else {
                continue;
            };
            let Some(wrapper) = node.stylo_data_mut() else {
                continue;
            };
            wrapper
                .borrow_mut()
                .hint
                .insert(RestyleHint::RESTYLE_CSS_ANIMATIONS | RestyleHint::RESTYLE_CSS_TRANSITIONS);
        }

        let mut marked = std::mem::take(&mut self.animations_mut().marked);
        marked.clear();
        for &id in hinted {
            let mut current = self.get(id).and_then(Node::flat_parent_id);
            while let Some(parent) = current {
                let Some(node) = self.get(parent) else { break };
                if node.has_animation_dirty_descendants() {
                    break;
                }
                node.set_animation_dirty_descendants_bit(true);
                marked.push(parent);
                current = node.flat_parent_id();
            }
        }
        self.animations_mut().marked = marked;
        self.document_element().id()
    }

    pub(super) fn recascade_animated_elements(&mut self, root: NodeId) -> AnimationTick {
        let now = self.animations().now();
        let animations = self.animations().context_handle();
        let phase = self.begin_flush_phase();
        let snapshots = SnapshotMap::new();
        let traversed = {
            let root_ref = self
                .get(root)
                .expect("the document element is never removed");
            let guards = self.style_engine().rule_guards();
            let shared = SharedStyleContext {
                stylist: self.style_engine().stylist(),
                visited_styles_enabled: false,
                options: StyleSystemOptions::default(),
                guards: guards.guards(),
                current_time_for_animations: now,
                traversal_flags: TraversalFlags::AnimationOnly,
                snapshot_map: &snapshots,
                animations,
                registered_speculative_painters: &NO_PAINTERS,
            };
            let traversal = RecalcStyle::new(shared, self.arenas().container_units_flag());
            let token = <RecalcStyle<'_> as stylo::traversal::DomTraversal<&Node<T>>>::pre_traverse(
                root_ref,
                traversal.shared(),
            );
            if token.should_traverse() {
                let _thread_state = LayoutThreadStateGuard::enter();
                Some(Node::id(driver::traverse_dom(&traversal, token, None)))
            } else {
                None
            }
        };
        drop(phase);

        let Some(traversed) = traversed else {
            self.clear_animation_marks();
            return AnimationTick::default();
        };
        let tick = self.harvest_animation_damage(traversed);
        self.clear_animation_marks();
        tick
    }

    fn clear_animation_marks(&mut self) {
        let marked = std::mem::take(&mut self.animations_mut().marked);
        for &id in &marked {
            if let Some(node) = self.get(id) {
                node.set_animation_dirty_descendants_bit(false);
            }
        }
        self.animations_mut().marked = marked;
    }

    fn harvest_animation_damage(&mut self, root: NodeId) -> AnimationTick {
        let mut tick = AnimationTick::default();
        let mut containing_blocks = Vec::new();
        let mut anchor_restyled = Vec::new();
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let (harvested, refresh, svg) = {
                let (harvested, refresh, descend, anchor_restyle, svg) = {
                    let Some(node) = self.arenas_mut().get_mut(current) else {
                        continue;
                    };
                    let damage = node.stylo_data_mut().map(|wrapper| {
                        let mut data = wrapper.borrow_mut();
                        let damage = data.damage;
                        data.clear_restyle_flags_and_damage();
                        damage
                    });
                    let refresh = node.refresh_layout_style();
                    let anchor_restyle = refresh.changed.then(|| {
                        node.layout_computed_style()
                            .is_some_and(crate::layout::anchors::declares_anchor_state)
                    });
                    let harvested = StyleDamage::from_style_change(
                        damage.unwrap_or_default(),
                        refresh.paragraph_limits_changed,
                    );
                    (
                        harvested,
                        refresh,
                        node.has_animation_dirty_descendants() || refresh.changed,
                        anchor_restyle,
                        node.in_svg_namespace(),
                    )
                };
                if let Some(declares) = anchor_restyle
                    && (declares || self.arenas().anchors().knows(current))
                {
                    anchor_restyled.push(current);
                }
                if descend {
                    let node = self
                        .arenas()
                        .get(current)
                        .expect("the node was live one statement ago");
                    let arenas = self.arenas();
                    stack.extend(
                        node.flat_children()
                            .iter()
                            .map(|&slot| arenas.at(slot).id()),
                    );
                }
                (harvested, refresh, svg)
            };
            self.note_box_restyle(
                current,
                refresh,
                harvested.is_some_and(StyleDamage::needs_relayout),
            );
            let Some(damage) = harvested else {
                continue;
            };
            tick.restyled += 1;
            let boxless = svg && self.note_svg_restyle(current);
            if refresh.containing_block_changed {
                containing_blocks.push(current);
            }
            if damage.needs_relayout() && !boxless {
                tick.relayout = true;
                self.invalidate_layout(current);
            }
        }
        for id in containing_blocks {
            tick.relayout |= self.invalidate_containing_block(id);
        }
        tick.relayout |= self.refresh_anchor_state(
            &anchor_restyled,
            crate::layout::anchors::RestyleSource::Animation,
        );
        let backdrops = self.restyle_backdrops();
        tick.restyled += backdrops.restyled;
        tick.relayout |= backdrops.relayout;
        tick.relayout |= self.rebuild_anonymous_boxes();
        tick
    }
}

#[cfg(test)]
mod tests {
    use stylo::dom::OpaqueNode;
    use stylo::servo::animation::{AnimationSetKey, AnimationState};

    use crate::style::engine::StylesheetOrigin;
    use crate::test_common::Doc;
    use crate::tree::document::tests::device;
    use crate::tree::document::{Document, NodeId};

    const SIDE_EFFECTS: &str = "
        page { display: flex; width: 800px; height: 600px; }
        .card { width: 100px; height: 100px; }
        @keyframes slide { from { transform: translateX(0px); }
                           to { transform: translateX(100px); } }
        @keyframes fade { from { opacity: 1; } to { opacity: 0; } }
        @keyframes recolor { from { background-color: red; }
                             to { background-color: blue; } }";

    fn animated_card(inline: &str) -> (Doc, NodeId) {
        let mut doc = Doc::with_css(SIDE_EFFECTS);
        let root = doc.root;
        let card = doc.el(root, "view.card");
        doc.set_inline(card, inline);
        doc.flush();
        (doc, card)
    }

    fn animates(doc: &Doc, id: NodeId) -> (bool, bool) {
        let node = doc.dom.get(id).expect("the card is live");
        (node.animates_opacity(), node.animates_transform())
    }

    #[test]
    fn the_animates_bits_follow_current_and_in_effect_animations() {
        for (inline, at, expected) in [
            ("animation: slide 1s linear 1s", 0.5, (false, true)),
            ("animation: fade 1s linear", 0.5, (true, false)),
            ("animation: recolor 1s linear", 0.5, (false, false)),
            (
                "animation: slide 1s linear, fade 1s linear",
                0.5,
                (true, true),
            ),
            ("animation: slide 1s linear paused", 0.5, (false, true)),
            ("animation: slide 1s linear", 1.5, (false, false)),
            ("animation: slide 1s linear forwards", 1.5, (false, true)),
            ("animation: fade 1s linear both", 1.5, (true, false)),
            ("animation: slide 1s linear backwards", 1.5, (false, false)),
        ] {
            let (mut doc, card) = animated_card(inline);
            doc.dom.advance_animations(0.0);
            doc.dom.advance_animations(at);
            doc.flush();
            assert_eq!(animates(&doc, card), expected, "{inline} at {at}s");
        }
    }

    #[test]
    fn cancellation_and_transitions_move_the_animates_bits() {
        let (mut doc, card) = animated_card("animation: slide 10s linear");
        doc.dom.advance_animations(0.0);
        assert_eq!(animates(&doc, card), (false, true));
        doc.set_inline(card, "animation: none");
        doc.flush();
        assert_eq!(animates(&doc, card), (false, false), "canceled");

        let (mut doc, card) = animated_card("opacity: 1; transition: opacity 1s linear");
        doc.set_inline(card, "opacity: 0; transition: opacity 1s linear");
        doc.flush();
        assert_eq!(animates(&doc, card), (true, false), "pending");
        doc.dom.advance_animations(0.0);
        doc.dom.advance_animations(0.5);
        assert_eq!(animates(&doc, card), (true, false), "running");
        doc.dom.advance_animations(1.5);
        doc.flush();
        assert_eq!(animates(&doc, card), (false, false), "finished");
    }

    #[test]
    fn unlinking_clears_the_animates_bits() {
        let (mut doc, card) = animated_card("animation: fade 10s linear");
        assert_eq!(animates(&doc, card), (true, false));
        doc.dom.remove_element(card);
        assert_eq!(animates(&doc, card), (false, false));
    }

    fn needs_layout(doc: &Doc) -> bool {
        let viewport = doc.dom.viewport_size();
        doc.dom.layout_needs_pass(
            seldon::geometry::Size::new(viewport.width, viewport.height),
            doc.dom.device().device_pixel_ratio().get(),
        )
    }

    #[test]
    fn a_transform_bit_flip_relayouts_once_not_per_frame() {
        let mut doc = Doc::with_css(&format!(
            "{SIDE_EFFECTS}
             .card {{ transform: translateX(0px); }}
             .slide {{ animation: slide 1s linear 0.5s; }}
             .fixed {{ position: fixed; left: 0; top: 0; width: 10px; height: 10px; }}"
        ));
        let root = doc.root;
        let card = doc.el(root, "view.card");
        let fixed = doc.el(card, "view.fixed");
        doc.flush();
        assert!(!needs_layout(&doc));
        assert_eq!(doc.dom.layout_cache_is_empty(fixed), Some(false));

        doc.add_class(card, "slide");
        doc.dom.flush_styles_with_damage_sink(&mut |_, _| {});
        assert_eq!(
            doc.dom.layout_cache_is_empty(fixed),
            Some(true),
            "the start invalidates the fixed child"
        );
        doc.flush();
        for now in [0.0, 0.25, 0.75, 1.0, 1.25] {
            let tick = doc.dom.advance_animations(now);
            assert!(
                !tick.relayout && !needs_layout(&doc),
                "no relayout at {now}s"
            );
            doc.flush();
        }
        let tick = doc.dom.advance_animations(2.0);
        assert!(tick.relayout && needs_layout(&doc), "the end relayouts");
        assert_eq!(doc.dom.layout_cache_is_empty(fixed), Some(true));
        doc.flush();
        assert_eq!(animates(&doc, card), (false, false));
    }

    #[test]
    fn a_transform_bit_flip_without_positioned_descendants_relayouts_nothing() {
        let css = format!(
            "{SIDE_EFFECTS}
             .card {{ transform: translateX(0px); }}
             .eased {{ transition: transform 1s linear; }}
             .moved {{ transform: translateX(50px); }}
             .flow {{ width: 10px; height: 10px; }}"
        );
        let mut doc = Doc::with_css(&css);
        let root = doc.root;
        let card = doc.el(root, "view.card.eased");
        doc.el(card, "view.flow");
        doc.flush();
        doc.add_class(card, "moved");
        doc.dom.flush_styles_with_damage_sink(&mut |_, _| {});
        assert_eq!(animates(&doc, card), (false, true), "the transition runs");
        assert!(!needs_layout(&doc), "its start relayouts nothing");
        doc.flush();
        doc.dom.advance_animations(0.0);
        let tick = doc.dom.advance_animations(1.5);
        assert_eq!(animates(&doc, card), (false, false), "the transition ended");
        assert!(
            !tick.relayout && !needs_layout(&doc),
            "the transition's end relayouts nothing"
        );

        let mut doc = Doc::with_css(&css);
        let root = doc.root;
        let card = doc.el(root, "view.card");
        doc.el(card, "view.flow");
        doc.set_inline(card, "animation: slide 1s linear");
        doc.flush();
        doc.dom.advance_animations(0.0);
        let tick = doc.dom.advance_animations(1.5);
        assert_eq!(animates(&doc, card), (false, false), "the animation ended");
        assert!(
            !tick.relayout && !needs_layout(&doc),
            "the animation's end relayouts nothing"
        );
    }

    #[test]
    fn a_transition_the_cancel_recascade_starts_sets_the_bits_at_once() {
        for (base, animation, expected) in [
            (
                "opacity: 0.5; transition: opacity 1s linear",
                "animation: pulse 10s linear",
                (true, false),
            ),
            (
                "transform: translateX(100px); transition: transform 1s linear",
                "animation: rest 10s linear",
                (false, true),
            ),
        ] {
            let mut doc = Doc::with_css(&format!(
                "{SIDE_EFFECTS}
                 @keyframes pulse {{ from {{ opacity: 1; }} to {{ opacity: 0.2; }} }}
                 @keyframes rest {{ from {{ transform: none; }}
                                    to {{ transform: translateX(10px); }} }}"
            ));
            let root = doc.root;
            let card = doc.el(root, "view.card");
            doc.set_inline(card, &format!("{base}; {animation}"));
            doc.flush();
            doc.set_inline(card, base);
            doc.flush();
            assert_eq!(animates(&doc, card), expected, "{animation} cancelled");
            assert!(
                doc.dom.has_active_animations(),
                "{animation}: the transition ticks"
            );
        }
    }

    fn transform_of(doc: &Doc, id: NodeId) -> String {
        format!("{:?}", doc.style(id).get_box().transform)
    }

    fn first_animation(doc: &Doc, id: NodeId) -> (AnimationState, f64) {
        let handle = doc.dom.animations().context_handle();
        let sets = handle.sets.read();
        let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(id.arena_key()));
        let animation = &sets.get(&key).expect("the card animates").animations[0];
        (animation.state.clone(), animation.started_at)
    }

    #[test]
    fn a_clock_sync_steps_states_without_a_recascade() {
        let (mut doc, card) = animated_card("animation: slide 1s linear 3");
        doc.dom.advance_animations(0.0);
        doc.dom.render();
        let committed = transform_of(&doc, card);
        for now in [0.5, 1.25] {
            doc.dom.sync_animation_clock(now);
            assert_eq!(doc.dom.animations().now(), now, "the clock moved");
            assert!(!doc.dom.needs_render(), "no commit is owed at {now}s");
            assert_eq!(
                transform_of(&doc, card),
                committed,
                "no re-cascade at {now}s"
            );
        }
        assert_eq!(first_animation(&doc, card).0, AnimationState::Running);
        doc.dom.sync_animation_clock(1.0);
        assert_eq!(
            doc.dom.animations().now(),
            1.25,
            "the clock never goes back"
        );

        let tick = doc.dom.advance_animations(1.25);
        assert!(
            tick.restyled > 0,
            "the tick re-cascades what the sync moved"
        );
        assert!(doc.dom.needs_render());
        assert_ne!(transform_of(&doc, card), committed);
    }

    #[test]
    fn a_tick_after_a_clock_sync_owes_the_commit_the_sync_deferred() {
        for synced in [false, true] {
            let (mut doc, _) = animated_card("animation: slide 1s linear 3");
            doc.dom.advance_animations(0.0);
            doc.dom.advance_animations(0.25);
            doc.dom.render();
            if synced {
                doc.dom.sync_animation_clock(1.25);
                assert!(!doc.dom.needs_render(), "the sync owes nothing yet");
            }
            let tick = doc.dom.advance_animations(1.25);
            assert_eq!(tick.restyled, 0, "the sample at 1.25 equals 0.25's");
            assert!(
                doc.dom.needs_render(),
                "the iteration owes a commit (synced: {synced})"
            );
        }
    }

    #[test]
    fn an_idle_tick_behind_a_sync_keeps_the_clock() {
        let (mut doc, _) = animated_card("");
        doc.dom.sync_animation_clock(1.012);
        doc.dom.advance_animations(1.0);
        assert_eq!(doc.dom.animation_clock(), 1.012);
    }

    #[test]
    fn a_clock_sync_that_ends_an_animation_recascades_it() {
        let (mut doc, card) = animated_card("animation: slide 1s linear");
        doc.dom.advance_animations(0.0);
        doc.dom.advance_animations(0.5);
        doc.dom.render();
        let running = transform_of(&doc, card);
        doc.dom.sync_animation_clock(1.5);
        assert_eq!(animates(&doc, card), (false, false), "the bit flipped");
        assert!(doc.dom.needs_render(), "the flip owes a commit");
        assert_ne!(transform_of(&doc, card), running, "the end re-cascaded");
        assert!(!doc.dom.has_active_animations());
    }

    #[test]
    fn a_clock_sync_leaves_a_fresh_start_to_the_next_tick() {
        let (mut doc, card) = animated_card("");
        doc.dom.advance_animations(0.0);
        doc.dom.sync_animation_clock(0.2);
        doc.set_inline(card, "animation: slide 1s linear");
        doc.flush();
        assert_eq!(first_animation(&doc, card), (AnimationState::Pending, 0.2));
        doc.dom.sync_animation_clock(0.5);
        assert_eq!(
            first_animation(&doc, card),
            (AnimationState::Pending, 0.5),
            "carried to the clock, not started"
        );
        doc.dom.advance_animations(0.75);
        assert_eq!(
            first_animation(&doc, card),
            (AnimationState::Running, 0.75),
            "anchored at the tick"
        );
    }

    #[test]
    fn an_animation_ending_without_a_restyle_still_invalidates_the_frame() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { width: 100px; height: 100px; animation: hold 0.1s linear; }
             @keyframes hold { from { opacity: 1; } to { opacity: 1; } }",
            StylesheetOrigin::Author,
        );
        document.render();
        let frame = document.committed_frame().expect("a frame is committed");
        assert!(
            frame.animations_active(),
            "the animation armed at the flush"
        );

        document.advance_animations(0.0);
        let tick = document.advance_animations(10.0);
        assert!(!tick.needs_next_frame, "the animation is over");
        assert!(
            document.needs_render(),
            "the idle transition must reach the next committed frame"
        );
        document.render();
        assert!(
            !document
                .committed_frame()
                .expect("a frame is committed")
                .animations_active(),
            "the committed frame reports the timeline idle"
        );
    }
}
