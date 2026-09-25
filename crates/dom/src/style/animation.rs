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
//! starts, on a per-frame `BeginFrame` command that carries the presenting
//! side's clock reading — so advancing an animation costs no script and no
//! DOM mutation; starting and stopping an animation rides the style flush
//! the same thread already runs. Elements whose animated
//! properties do not affect geometry never reach layout either, because the
//! harvest only calls `invalidate_layout` for damage that
//! [`StyleDamage::needs_relayout`] reports.
//!
//! This crate owns no clock (see [`crate::input`]): `now` is a parameter. The
//! timeline therefore only moves in [`Document::advance_animations`], and the
//! presenting side is free to stop calling it — an idle page sends no frame,
//! and an animation an exported curve already covers is sampled on the
//! painting side instead. The flush that creates an animation consequently
//! reads whatever time the last tick left behind, which can be arbitrarily far
//! in the past.
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

use rustc_hash::{FxHashMap, FxHashSet};
use stylo::context::{SharedStyleContext, StyleSystemOptions};
use stylo::dom::OpaqueNode;
use stylo::driver;
use stylo::invalidation::element::restyle_hints::RestyleHint;
use stylo::properties::longhands::animation_fill_mode::computed_value::single_value::T as AnimationFillMode;
use stylo::properties::{
    LonghandId, OwnedPropertyDeclarationId, PropertyDeclarationId, PropertyDeclarationIdSet,
};
use stylo::selector_parser::SnapshotMap;
use stylo::servo::animation::{
    Animation, AnimationSetKey, AnimationState, DocumentAnimationSet, ElementAnimationSet,
    Transition,
};
use stylo::shared_lock::StylesheetGuards;
use stylo::traversal_flags::TraversalFlags;
use stylo_atoms::Atom;

use crate::layout::skips_contents;
use crate::style::damage::StyleDamage;
use crate::style::flush::{LayoutThreadStateGuard, NO_PAINTERS, RecalcStyle};
use crate::tree::document::{Document, NodeId};
use crate::tree::node::{ANIMATES_OPACITY, ANIMATES_TRANSFORM, Node};

/// What one [`Document::advance_animations`] call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimationTick {
    /// An animation is still live and the caller owes it another frame. False
    /// once every animation has finished, been canceled, or paused.
    pub needs_next_frame: bool,
    /// How many elements were re-cascaded. Zero means the retained scene is
    /// still current and this frame can be skipped.
    pub restyled: usize,
    /// An animated element produced relayout damage, or a transform
    /// animation starting or ending moved the containing block of its
    /// element's positioned descendants, so the next `layout()` will do real
    /// work. False on every other tick of a purely visual animation.
    pub relayout: bool,
}

fn is_cancelled(animation: &impl Cancellable) -> bool {
    animation.is_cancelled()
}

/// Whether one animation or transition has been cancelled.
///
/// Stylo's `Animation` and `Transition` carry the same `state` field but share
/// no trait, so the two loops that ask this question need one of their own.
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

/// The longhands whose animation acts as `will-change` naming a composite
/// property, and the [`crate::tree::node::StylingData::animates`] bit each
/// sets. The individual transforms are storage-only in the fork's grammar.
const COMPOSITES: [(LonghandId, u8); 5] = [
    (LonghandId::Opacity, ANIMATES_OPACITY),
    (LonghandId::Transform, ANIMATES_TRANSFORM),
    (LonghandId::Translate, ANIMATES_TRANSFORM),
    (LonghandId::Rotate, ANIMATES_TRANSFORM),
    (LonghandId::Scale, ANIMATES_TRANSFORM),
];

/// web-animations-1 "Side effects of animation": whether a CSS animation is
/// *current* or *in effect*. Stylo runs every animation at a positive
/// playback rate, so its before phase (`Pending`, a delay included) and a
/// pause are current; a finished one stays in effect only while its fill
/// holds the last keyframe.
fn animation_has_side_effects(animation: &Animation) -> bool {
    match animation.state {
        AnimationState::Pending | AnimationState::Running | AnimationState::Paused(_) => true,
        AnimationState::Finished => matches!(
            animation.fill_mode,
            AnimationFillMode::Forwards | AnimationFillMode::Both
        ),
        AnimationState::Canceled => false,
    }
}

/// The same for a CSS transition, which fills nothing once it finishes.
fn transition_has_side_effects(transition: &Transition) -> bool {
    matches!(
        transition.state,
        AnimationState::Pending | AnimationState::Running
    )
}

fn composite_bits(properties: &PropertyDeclarationIdSet) -> u8 {
    COMPOSITES
        .iter()
        .filter(|(longhand, _)| properties.contains(PropertyDeclarationId::Longhand(*longhand)))
        .fold(0, |bits, (_, bit)| bits | bit)
}

fn composite_bit(property: PropertyDeclarationId<'_>) -> u8 {
    COMPOSITES
        .iter()
        .find(|(longhand, _)| property.as_longhand() == Some(*longhand))
        .map_or(0, |(_, bit)| *bit)
}

/// What one step of Stylo's animation state machine produced.
#[derive(Default)]
struct Stepped {
    /// The elements whose animated values may have moved, for the
    /// animation-only traversal to re-cascade.
    hinted: Vec<NodeId>,
    /// Whether any animation or transition changed state — started, iterated
    /// or ended. The committed frame describes those states as well as the
    /// styles they produce, so a step that moved one owes the next frame a
    /// commit even if no style changed with it.
    moved: bool,
}

/// One animation or transition the driver has already anchored to the
/// timeline and which was still `Pending` when the last tick ended, waiting
/// out its delay.
///
/// Stylo's own `is_new` field answers a different question:
/// `Animation::update_from_other` assigns the whole animation over, so a
/// restyle of the element sets `is_new` again and a delayed animation on an
/// element that restyles every frame would be pushed forward forever. The
/// identity below survives that, because a restyle keeps both the name and the
/// transitioned property.
#[derive(PartialEq, Eq, Hash)]
struct AnchoredAnimation {
    /// The map key the animation lives under, so an element and one of its
    /// pseudo-elements running the same animation name stay distinct.
    set: AnimationSetKey,
    what: AnchoredKind,
}

/// Which animation of a set [`AnchoredAnimation`] names.
#[derive(PartialEq, Eq, Hash)]
enum AnchoredKind {
    /// A `@keyframes` animation, by its `animation-name`.
    Keyframes(Atom),
    /// A transition, by the property it animates — the same identity Stylo
    /// matches a replacement transition against.
    Transition(OwnedPropertyDeclarationId),
}

/// One element's share of a tick: the instant, the interval, whether the
/// element is frozen, and the anchoring bookkeeping the two loops share.
///
/// Split out of [`Document::step_animation_states`] only so the animation and
/// transition loops — the same five decisions over two Stylo types that share
/// no trait — read side by side.
struct Step<'a> {
    now: f64,
    shift: f64,
    /// css-contain-2 §4: this element is in a skipped subtree, so nothing is
    /// promoted, iterated or ended for it.
    skipped: bool,
    /// Whether this element's start times take [`Self::shift`] whatever their
    /// state, which is what holds a frozen animation's progress still.
    carry: bool,
    anchored: &'a FxHashSet<AnchoredAnimation>,
    pending: &'a mut FxHashSet<AnchoredAnimation>,
}

impl Step<'_> {
    /// Whether a `Pending` animation or transition takes the interval now,
    /// `fresh` when this tick is the first to see it.
    ///
    /// Carry and anchor are exclusive by construction: the shift lands once,
    /// from whichever of the two claims it, so an animation created inside a
    /// skipped subtree is not pushed forward twice on the tick that reveals
    /// it.
    const fn shifts_pending(&self, fresh: bool) -> bool {
        self.carry || fresh
    }

    /// Whether this tick settles an animation's start on an element that is
    /// not frozen: it anchors a `fresh` `Pending` one, or carries the starts
    /// of an element revealed since the last tick. A curve exports only
    /// settled starts, so the step owes the next frame a commit.
    const fn settles(&self, fresh: bool) -> bool {
        !self.skipped && (fresh || self.carry)
    }

    /// Advances one element's `@keyframes` animations, collecting the ones
    /// that finished with a fill mode that keeps their last value. Answers
    /// whether any of them changed state.
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

    /// The same for one element's transitions, which have no iteration and no
    /// fill mode to hold.
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
                if !self.skipped && transition.start_time <= self.now {
                    transition.state = AnimationState::Running;
                    moved = true;
                } else {
                    self.pending.insert(entry);
                }
            } else if self.carry {
                transition.start_time += self.shift;
            }
            if self.skipped {
                continue;
            }
            if transition.state == AnimationState::Running && transition.has_ended(self.now) {
                transition.state = AnimationState::Finished;
                moved = true;
            }
        }
        moved
    }
}

/// The document's animation timeline: Stylo's animation state, the last time
/// it was sampled at, and whether anything is still moving.
#[derive(Default)]
pub(crate) struct AnimationDriver {
    sets: DocumentAnimationSet,
    now: f64,
    active: bool,
    /// Whether any element that owns animation state sits in a skipped
    /// subtree, so its animations are frozen (css-contain-2 §4, module docs).
    ///
    /// Kept beside [`Self::active`] rather than folded into it because the two
    /// answer different questions: a frozen animation demands no frame of its
    /// own — that is the whole point — and yet its start times still have to
    /// be carried every time the timeline moves.
    frozen: bool,
    /// The elements whose animations were frozen when the last tick ended.
    ///
    /// A tick carries the start times of every element in here *or* frozen as
    /// of the tick itself, which is what makes the interval a reveal happened
    /// in count as frozen rather than as elapsed. See the module docs: the
    /// reveal is noticed between two ticks and the interval containing it may
    /// be an arbitrarily long unticked stretch.
    carried: FxHashSet<NodeId>,
    /// The set the previous tick's [`Self::carried`] is rebuilt into, so a
    /// page with a frozen animation ticks without allocating.
    carried_spare: FxHashSet<NodeId>,
    /// Ancestors marked with the animation-only dirty-descendants bit by the
    /// last tick, kept so the same tick can clear exactly what it set. Stylo
    /// only ever sets that bit on ancestors of a hinted element, so this is a
    /// superset of what the traversal touches.
    marked: Vec<NodeId>,
    /// Elements currently carrying the `may_have_animations` bit, so the bit
    /// can be cleared again when their last animation goes away.
    flagged: Vec<NodeId>,
    /// The animations and transitions that were already anchored to the
    /// timeline and still `Pending` at the end of the last tick, so the next
    /// tick shifts the ones it has never seen and leaves these alone.
    ///
    /// A set rather than a list: a page of staggered delays holds one entry per
    /// waiting animation for its whole delay, and every tick asks about each of
    /// them, which over a list is one pass per animation per frame.
    anchored: FxHashSet<AnchoredAnimation>,
    /// The set the previous tick's [`Self::anchored`] is rebuilt into, kept so
    /// a tick of a page with a delayed animation allocates nothing.
    anchored_spare: FxHashSet<AnchoredAnimation>,
    /// Animations that have finished but whose fill mode keeps their last
    /// value in the cascade.
    ///
    /// Stylo drops every finished animation from its map on the next restyle
    /// (`process_animations_for_style`), which would take the held value with
    /// it. A browser keeps a filling animation contributing until it is
    /// cancelled or replaced, so the driver puts them back after each
    /// traversal.
    held: Vec<(NodeId, Animation)>,
}

impl AnimationDriver {
    /// The handle a [`SharedStyleContext`] takes. Cloning shares the same
    /// `Arc<RwLock<..>>`, which is how the state survives a flush.
    pub(crate) fn context_handle(&self) -> DocumentAnimationSet {
        self.sets.clone()
    }

    /// Whether the pending `@keyframes` animation `name` of set `set` is
    /// anchored to the timeline, so the next tick leaves its start alone.
    pub(crate) fn keyframes_anchored(&self, set: &AnimationSetKey, name: &Atom) -> bool {
        self.anchored.contains(&AnchoredAnimation {
            set: set.clone(),
            what: AnchoredKind::Keyframes(name.clone()),
        })
    }

    /// Whether the next tick carries `id`'s start times: it was frozen when
    /// the last tick ended.
    pub(crate) fn carries(&self, id: NodeId) -> bool {
        self.carried.contains(&id)
    }

    /// The time the animations were last sampled at, in seconds.
    pub(crate) const fn now(&self) -> f64 {
        self.now
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.active
    }

    /// Whether a tick still has work even with nothing active: something is
    /// frozen right now, or something was frozen when the last tick ended and
    /// its start times have not been carried across the interval since.
    ///
    /// Both terms matter. The first is what keeps a frozen animation's
    /// progress from drifting while an unrelated animation keeps the host
    /// ticking; the second is what lets the tick after a reveal carry the
    /// interval the reveal happened in, however long the host left it.
    pub(crate) fn has_frozen_animations(&self) -> bool {
        self.frozen || !self.carried.is_empty()
    }

    /// Whether the document holds no animation state at all — the check that
    /// keeps removal and unlinking free on a page that never animates.
    pub(crate) fn is_empty(&self) -> bool {
        self.sets.sets.read().is_empty()
    }

    /// Drops every animation belonging to the given nodes.
    ///
    /// Two callers need this. Unlinking an element cancels its animations, the
    /// same as a browser restarting them when a node is moved; and freeing an
    /// arena slot must not hand a dead element's animations to whatever reuses
    /// the slot, because [`stylo::dom::OpaqueNode`] carries the bare slot with
    /// the generation stripped and `DocumentAnimationSet` reaps nothing on its
    /// own.
    pub(crate) fn forget(&mut self, ids: &[NodeId]) {
        if ids.is_empty() {
            return;
        }
        self.flagged.retain(|flagged| !ids.contains(flagged));
        self.marked.retain(|marked| !ids.contains(marked));
        self.carried.retain(|carried| !ids.contains(carried));
        self.held.retain(|(id, _)| !ids.contains(id));
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
        // This can retire the last animation in the document, and nothing
        // guarantees a style flush follows to notice. Deliberately blind to
        // freezing, which needs the tree this half of the driver cannot
        // reach: the answer is conservative in the safe direction, and
        // `Document::refresh_animation_activity` corrects it before the next
        // frame is published.
        self.active = sets
            .values()
            .any(stylo::servo::animation::ElementAnimationSet::needs_animation_ticks);
    }
}

impl<T: Sync> Document<T> {
    /// Whether any animation or transition still needs frames.
    ///
    /// Animations frozen by css-contain-2 §4 — the ones in a subtree whose
    /// contents are skipped — do not count: they advance no timeline, so a
    /// page whose only animations are frozen is idle and the host stops
    /// ticking it. See the module docs.
    #[must_use]
    pub fn has_active_animations(&self) -> bool {
        self.animations().is_active()
    }

    /// css-contain-2 §4: whether this element is part of some ancestor's
    /// **skipped contents**, which is what freezes its animations.
    ///
    /// The spec skips an element's *contents* — "the flat tree descendants of
    /// the element, including both text and elements" — so the box that skips
    /// is itself not skipped and its own animations run as normal. Hence the
    /// walk starts at the flat parent.
    ///
    /// The predicate is `crate::layout::skips_contents`, the one layout and
    /// the paint build ask, so `content-visibility: hidden` and a non-relevant
    /// `content-visibility: auto` box freeze by exactly the rule they skip by,
    /// and the answer moves in the same rendering update the skipping does.
    /// The walk is per *animated* element and costs a page with no animation
    /// state nothing at all, because nothing asks.
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

    /// Re-reads which animations demand frames and which are frozen, and
    /// answers the former.
    ///
    /// Two things move this answer without a tick: a style flush, which is
    /// where animations start, stop and gain or lose a skipping ancestor; and
    /// the `content-visibility: auto` relevance pass inside
    /// [`Document::render`], which reveals or skips subtrees with no style
    /// change at all. So the render asks again once its frame is final, which
    /// is what makes a reveal start its animations in the very commit that
    /// revealed them.
    pub(crate) fn refresh_animation_activity(&mut self) -> bool {
        let handle = self.animations().context_handle();
        let (active, frozen) = {
            let sets = handle.sets.read();
            let mut active = false;
            let mut frozen = false;
            for (key, set) in &*sets {
                let Some(id) = self.arenas().id_at_arena_key(key.node.0) else {
                    continue;
                };
                if self.in_skipped_subtree(id) {
                    frozen = true;
                } else {
                    active |= set.needs_animation_ticks();
                }
            }
            (active, frozen)
        };
        let driver = self.animations_mut();
        driver.active = active;
        driver.frozen = frozen;
        active
    }

    /// Whether anything animating is *not* covered by one of `frame`'s
    /// exported curves — those elements still need per-frame ticks on this
    /// thread, so the presenting side keeps sending `BeginFrame`s.
    ///
    /// Every exported element has exactly one set, found by key, so counting
    /// the covered sets answers without scanning the slots per set.
    pub(crate) fn animation_needs_main_ticks(&self, frame: &crate::visual::PaintOrder) -> bool {
        let handle = self.animations().context_handle();
        let sets = handle.sets.read();
        let arenas = self.arenas();
        let ticks = |key: &AnimationSetKey, set: &ElementAnimationSet| {
            // A frozen element's animations do not advance, and it has no
            // curve either: the build never descends into a skipped subtree.
            set.needs_animation_ticks()
                && arenas
                    .id_at_arena_key(key.node.0)
                    .is_some_and(|id| !self.in_skipped_subtree(id))
        };
        let ticking = sets.iter().filter(|(key, set)| ticks(key, set)).count();
        let covered = frame
            .animations()
            .iter()
            .filter(|slot| {
                let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(slot.node.arena_key()));
                sets.get_key_value(&key)
                    .is_some_and(|(key, set)| ticks(key, set))
            })
            .count();
        ticking > covered
    }

    /// Advances every live animation and transition to `now` — seconds on a
    /// monotonic timeline whose epoch is the caller's choice — and re-cascades
    /// the elements that moved.
    ///
    /// Safe to call every frame: with nothing animating it reads one bool.
    /// Time never runs backwards; a `now` behind the last sample is clamped
    /// forward rather than rewinding the timeline.
    ///
    /// The interval between this sample and the last one is also what anchors
    /// an animation created since the last one (see the module docs), which is
    /// why the previous reading is read before it is overwritten. The inactive
    /// path records the reading without stepping anything: a flush that creates
    /// an animation makes the timeline active through
    /// [`Document::sync_animation_state`], so the first tick that can see a new
    /// animation always takes the path below, with `previous` at the time the
    /// animation was created.
    ///
    /// A document with nothing active but something *frozen* takes the path
    /// below too, and for the same reason the anchoring exists: a frozen
    /// animation's start times have to be carried by whatever interval the
    /// timeline moved, or its progress would drift while it was meant to be
    /// standing still. That step promotes, iterates, ends and hints nothing,
    /// so it leaves the timeline exactly as idle as it found it.
    pub fn advance_animations(&mut self, now: f64) -> AnimationTick {
        let was_active = self.animations().is_active();
        if !was_active && !self.animations().has_frozen_animations() {
            self.animations_mut().now = now;
            return AnimationTick::default();
        }
        let previous = self.animations().now;
        let now = now.max(previous);
        self.animations_mut().now = now;

        let Stepped { hinted, moved } = self.step_animation_states(now, now - previous);
        if hinted.is_empty() {
            let relayout = self.sync_animation_state();
            // If the timeline was active on entry and this step ended it, the
            // idle fact must reach the next committed frame even though no
            // style moved — the frame's animation flag is itself visual
            // state, and a stale `true` would keep the compositor asking
            // for animation ticks forever. The entry state is what makes that
            // a transition rather than a standing condition: a tick that only
            // carries frozen animations finds the timeline idle on both sides
            // and must not republish a frame for it, every frame, forever.
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
        // The traversal runs `process_animations` again, which prunes finished
        // animations, so what the timeline owns is only settled afterwards.
        tick.relayout |= self.sync_animation_state();
        tick.needs_next_frame = self.animations().is_active();
        // A restyle is a visual change; so is the timeline going idle, whose
        // flag rides the committed frame (see above). So is a state change on
        // its own: the exported curves the frame carries are read off these
        // states, and an animation promoted to running is one the next frame
        // can hand to the compositor even when its first sample happens to
        // equal the style already committed.
        if tick.restyled > 0 || moved || (was_active && !tick.needs_next_frame) {
            self.note_visual_mutation();
        }
        tick
    }

    /// Re-reads Stylo's animation map: which elements own animation state,
    /// which composite properties each animates (the node's `animates`
    /// bits), and whether anything still needs frames. Answers whether the
    /// next `layout()` has work: a transform bit flip moved a containing
    /// block, or the cancelled elements' re-cascade relayouts.
    ///
    /// Both the style flush and the tick mutate the map through Stylo, which
    /// reports neither, so the bookkeeping is rebuilt from the map itself.
    /// Whether an element's animations are *frozen* is the one question this
    /// pass does not answer itself: it goes through
    /// [`Document::refresh_animation_activity`] below, so freezing and the
    /// render's own re-ask read one rule.
    pub(crate) fn sync_animation_state(&mut self) -> bool {
        // The cancelled elements' re-cascade can start a transition from the
        // value the animation left, and prunes the finished animations a fill
        // holds, so everything below reads the map after it.
        let cancelled = self.drop_cancelled_animations();
        let mut relayout = !cancelled.is_empty() && self.recascade_cancelled_animations(&cancelled);

        let handle = self.animations().context_handle();
        let mut held = std::mem::take(&mut self.animations_mut().held);
        let mut animated = Vec::new();
        // Elements whose `animates` bits changed, with the bits that did.
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
            // An element whose set Stylo dropped since the last sync
            // animates nothing; the loop above never saw it.
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
        if !flipped.is_empty() {
            // Either bit moves the stacking context and group the paint build
            // gives the element.
            self.note_visual_mutation();
        }
        for (id, changed) in flipped {
            // `will-change: transform` makes the element the containing block
            // of its absolute and fixed descendants. Only an animation's start
            // or end gets here, never a frame.
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
        self.refresh_animation_activity();
        relayout
    }

    /// Puts the held animations back, then drops every cancelled animation
    /// and transition from the map, answering the elements that had one. The
    /// held ones go back first so that a cancelled one stops filling.
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

    /// The [`ANIMATES_OPACITY`] and [`ANIMATES_TRANSFORM`] bits of `node`'s
    /// own set: the composite properties its animations and transitions name
    /// while they have side effects. A keyframes animation's properties are
    /// its `@keyframes` rule's, the set Stylo built the animation from.
    fn animates(&self, node: &Node<T>, set: &ElementAnimationSet) -> u8 {
        let stylist = self.style_engine().stylist();
        let animations = set
            .animations
            .iter()
            .filter(|animation| animation_has_side_effects(animation))
            .filter_map(|animation| stylist.lookup_keyframes(&animation.name, node))
            .fold(0, |bits, keyframes| {
                bits | composite_bits(&keyframes.properties_changed)
            });
        set.transitions
            .iter()
            .filter(|transition| transition_has_side_effects(transition))
            .fold(animations, |bits, transition| {
                bits | composite_bit(transition.property_animation.property_id())
            })
    }

    /// Re-cascades elements whose animations a restyle just cancelled.
    ///
    /// Stylo cancels an animation the new style no longer names inside
    /// `ElementAnimationSet::update_animations_for_new_style`, which — unlike
    /// the two sibling cancel paths next to it — does not mark the set dirty,
    /// so `process_animations` never replaces the element's `Animations`
    /// cascade origin. The element is left holding the value the animation had
    /// when it was cancelled, for as long as nothing else restyles it. A
    /// browser drops straight back to the un-animated style, so the driver
    /// replaces that origin itself: the animation is out of the map by now, so
    /// `TElement::animation_rule` answers `None` and the origin goes away.
    /// Answers whether that produced relayout damage.
    fn recascade_cancelled_animations(&mut self, cancelled: &[NodeId]) -> bool {
        let root = self.hint_animated_elements(cancelled);
        let tick = self.recascade_animated_elements(root);
        if tick.restyled > 0 {
            self.note_visual_mutation();
        }
        tick.relayout
    }

    /// Puts back the finished-but-filling animations Stylo's restyle removed,
    /// and drops the ones a restyle cancelled or whose element is gone.
    ///
    /// `animation-fill-mode: forwards` means the last keyframe keeps applying
    /// after the animation ends, which is a statement about the cascade, not
    /// about the frame it ended on: it holds until the animation is cancelled
    /// or replaced. Stylo's `process_animations_for_style` retains only
    /// unfinished animations, so without this the held value survives exactly
    /// until the next restyle of that element.
    fn restore_held_animations(
        &self,
        held: &mut Vec<(NodeId, Animation)>,
        sets: &mut FxHashMap<AnimationSetKey, ElementAnimationSet>,
    ) {
        held.retain(|(id, animation)| {
            if self.arenas().get(*id).is_none() {
                return false;
            }
            let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(id.arena_key()));
            // A restyle whose `animation-name` no longer names this animation
            // cancels it, and a cancelled animation stops filling.
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

    /// Advances Stylo's animation state machine to `now` and returns the
    /// elements whose animated values may have moved.
    ///
    /// Stylo itself never writes [`AnimationState::Running`] outside
    /// `Animation::update_from_other`, and `iterate_to` refuses to advance an
    /// animation that is still `Pending`, so both the start promotion and the
    /// iteration belong to the driver. `iterate_to` takes every iteration a
    /// stalled frame skipped at once, by the arithmetic an exported curve's
    /// sampler repeats.
    ///
    /// `shift` is how far the timeline moved to reach `now`. Every `Pending`
    /// animation the driver has not anchored yet starts at this frame, so it
    /// carries that interval before the promotion below considers it.
    ///
    /// `shift` is also what freezes an element whose contents are skipped
    /// (css-contain-2 §4, module docs): carrying *every* one of its start
    /// times by the interval holds `now - started_at` still, which is the
    /// animation's progress, and the step then promotes, iterates, ends and
    /// hints nothing for it. An element carries when it is frozen now or was
    /// frozen when the last tick ended — the second term is what makes the
    /// interval a reveal happened in count as frozen, and it is the only one
    /// that survives the host having stopped ticking a fully frozen page.
    /// Carry and anchor are exclusive by construction: a `Pending` animation
    /// takes `shift` once, from whichever of the two claims it.
    fn step_animation_states(&mut self, now: f64, shift: f64) -> Stepped {
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
                // The element was freed without a lifecycle hook reaching us.
                return false;
            };
            let skipped = document.in_skipped_subtree(id);
            let mut step = Step {
                now,
                shift,
                skipped,
                // Carrying is what freezes: it applies while the element is
                // skipped, and once more on the tick that finds it revealed,
                // for the interval the reveal happened somewhere inside.
                carry: skipped || was_frozen.contains(&id),
                anchored: &anchored,
                pending: &mut pending,
            };
            if skipped {
                frozen.insert(id);
            }
            let mut moved = step.animations(id, key, &mut set.animations, &mut held);
            moved |= step.transitions(key, &mut set.transitions);
            stepped.moved |= moved;
            set.clear_canceled_animations();
            if set.is_empty() {
                return false;
            }
            // A frozen element is deliberately not hinted: its sampled values
            // cannot have moved, and re-cascading it every frame is exactly
            // the work skipping contents exists to avoid.
            if !skipped && (set.needs_animation_ticks() || moved) {
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

    /// Marks each animated element for an animation-only restyle and opens the
    /// path the traversal descends. Returns the traversal root.
    ///
    /// Stylo propagates no hint of its own during an animation-only traversal
    /// (`RestyleHint::propagate` returns empty and strips the animation bits),
    /// so descent is driven purely by the animation-only dirty-descendants
    /// bit, which the caller has to open from the root down.
    fn hint_animated_elements(&mut self, hinted: &[NodeId]) -> NodeId {
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

    /// Runs the animation-only traversal and harvests what it changed.
    fn recascade_animated_elements(&mut self, root: NodeId) -> AnimationTick {
        let now = self.animations().now();
        let animations = self.animations().context_handle();
        let phase = self.begin_flush_phase();
        // An animation-only traversal never looks at snapshots — `pre_traverse`
        // skips invalidation under the flag — and must not consume the pending
        // ones, which belong to the next normal flush.
        let snapshots = SnapshotMap::new();
        let traversed = {
            let root_ref = self
                .get(root)
                .expect("the document element is never removed");
            let guard = self.style_engine().shared_lock().read();
            let shared = SharedStyleContext {
                stylist: self.style_engine().stylist(),
                visited_styles_enabled: false,
                options: StyleSystemOptions::default(),
                guards: StylesheetGuards::same(&guard),
                current_time_for_animations: now,
                // Deliberately not `FinalAnimationTraversal`: that flag wipes
                // `ElementData::damage` before the harvest can read it.
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
                // Sequential: an animation touches a handful of elements, and
                // on Wasm worker zero of the Stylo pool is the Render
                // Worker — the thread that paints — which is not this one.
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

    /// Collects the damage an animation-only traversal produced.
    ///
    /// Mirrors the normal post-flush harvest with two deltas: descent follows
    /// the animation-only dirty-descendants bit rather than the normal one, so
    /// a pending non-animation restyle keeps its bits for the next real flush;
    /// and only damage is cleared, never the restyle hint, for the same
    /// reason. Descent through changed styles is what carries an inherited
    /// animated property — Stylo hands the subtree a recascade hint and the
    /// walk has to follow it.
    fn harvest_animation_damage(&mut self, root: NodeId) -> AnimationTick {
        let mut tick = AnimationTick::default();
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let harvested = {
                let (harvested, descend) = {
                    let Some(node) = self.arenas_mut().get_mut(current) else {
                        continue;
                    };
                    let damage = node.stylo_data_mut().map(|wrapper| {
                        let mut data = wrapper.borrow_mut();
                        let damage = data.damage;
                        data.clear_restyle_flags_and_damage();
                        damage
                    });
                    // Reads the primary style after the clear above, which is
                    // sound only because clearing restyle state touches the
                    // hint, the damage, and the flags — never `styles`.
                    let refresh = node.refresh_layout_style();
                    let harvested = StyleDamage::from_style_change(
                        damage.unwrap_or_default(),
                        refresh.paragraph_limits_changed,
                    );
                    (
                        harvested.map(|damage| (damage, refresh)),
                        node.has_animation_dirty_descendants() || refresh.changed,
                    )
                };
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
                harvested
            };
            let Some((damage, refresh)) = harvested else {
                continue;
            };
            tick.restyled += 1;
            if damage.needs_relayout() {
                tick.relayout = true;
                // An animated `font-size` reaches its text through this
                // harvest and no other, so the text children need the same
                // two-level invalidation the style harvest gives them.
                self.invalidate_text_children(current, refresh.shaping_changed);
                self.invalidate_layout(current);
            }
        }
        tick
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common::Doc;
    use crate::tree::document::tests::device;
    use crate::{Document, NodeId, StylesheetOrigin};

    const SIDE_EFFECTS: &str = "
        page { display: flex; width: 800px; height: 600px; }
        .card { width: 100px; height: 100px; }
        @keyframes slide { from { transform: translateX(0px); }
                           to { transform: translateX(100px); } }
        @keyframes fade { from { opacity: 1; } to { opacity: 0; } }
        @keyframes recolor { from { background-color: red; }
                             to { background-color: blue; } }";

    /// A `.card` running `inline`, created by a flush and started on the
    /// timeline's origin by the tick at 0.
    fn animated_card(inline: &str) -> (Doc, NodeId) {
        let mut doc = Doc::with_css(SIDE_EFFECTS);
        let root = doc.root;
        let card = doc.el(root, "view.card");
        doc.set_inline(card, inline);
        doc.flush();
        (doc, card)
    }

    /// `(animates_opacity, animates_transform)`.
    fn animates(doc: &Doc, id: NodeId) -> (bool, bool) {
        let node = doc.dom.get(id).expect("the card is live");
        (node.animates_opacity(), node.animates_transform())
    }

    /// web-animations-1 "Side effects of animation": the bits hold while an
    /// animation is current (its delay, a pause, while it runs) or in effect
    /// (a forwards fill), and name only what its `@keyframes` animate.
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

    /// A canceled animation and a finished transition leave no bits; a
    /// pending or running transition sets them.
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

    /// Unlinking cancels the animations and clears the bits with them.
    #[test]
    fn unlinking_clears_the_animates_bits() {
        let (mut doc, card) = animated_card("animation: fade 10s linear");
        assert_eq!(animates(&doc, card), (true, false));
        doc.dom.remove_element(card);
        assert_eq!(animates(&doc, card), (false, false));
    }

    /// Whether the next `layout()` does a pass.
    fn needs_layout(doc: &Doc) -> bool {
        let viewport = doc.dom.viewport_size();
        doc.dom.layout_needs_pass(
            hughie::geometry::Size::new(viewport.width, viewport.height),
            doc.dom.device().device_pixel_ratio().get(),
        )
    }

    /// The transform bit moves the containing block of the card's fixed
    /// child, so its start and end each invalidate that child once; no frame
    /// between them does. Adding the class changes `animation-*` longhands,
    /// which relayout the card by themselves, so the start reads the child's
    /// cache rather than whether a pass is due.
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

    /// With no absolute or fixed descendant no containing block moves, so a
    /// transform transition or animation relayouts nothing at its start or
    /// end, whatever its bit does. The card's own transform keeps the style
    /// from relayouting on its own.
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

    /// Cancelling an animation re-cascades its element once more, and that
    /// cascade can start a transition from the animated value. The bits and
    /// the timeline read the map after it, so the transition's side effects
    /// land in the same flush.
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
            // Pending until the first tick, the animation holds its first
            // keyframe, which the cancelling flush transitions from.
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

    /// The finishing tick must republish even when the final style equals the
    /// previous one: the committed frame's animation flag is what keeps the
    /// compositor asking for ticks, and a stale `true` would never end.
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

        // The flush arms the animation; the first tick is what starts it, so
        // the sample below is ten seconds into a 0.1s animation, not ten
        // seconds after a flush that happened at an unticked zero.
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
