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
//! presenting thread holds that between frames, so advancing an animation
//! costs no script, no DOM mutation, and no round trip to the Lynx main
//! thread; only *starting* and *stopping* an animation needs the main thread,
//! and that rides the style flush it already runs. Elements whose animated
//! properties do not affect geometry never reach layout either, because the
//! harvest only calls `invalidate_layout` for damage that
//! [`StyleDamage::needs_relayout`] reports.
//!
//! This crate owns no clock (see [`crate::input`]): `now` is a parameter.

use stylo::context::{SharedStyleContext, StyleSystemOptions};
use stylo::dom::OpaqueNode;
use stylo::driver;
use stylo::invalidation::element::restyle_hints::RestyleHint;
use stylo::properties::longhands::animation_fill_mode::computed_value::single_value::T as AnimationFillMode;
use stylo::selector_parser::SnapshotMap;
use stylo::servo::animation::{
    Animation, AnimationSetKey, AnimationState, DocumentAnimationSet, ElementAnimationSet,
    Transition,
};
use stylo::shared_lock::StylesheetGuards;
use stylo::traversal_flags::TraversalFlags;

use crate::style::damage::StyleDamage;
use crate::style::flush::{LayoutThreadStateGuard, NO_PAINTERS, RecalcStyle};
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

/// What one [`Document::advance_animations`] call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimationTick {
    /// An animation is still live and the caller owes it another frame. False
    /// once every animation has finished, been canceled, or paused.
    pub needs_next_frame: bool,
    /// How many elements were re-cascaded. Zero means the retained scene is
    /// still current and this frame can be skipped.
    pub restyled: usize,
    /// An animated element produced relayout damage, so the next `layout()`
    /// will do real work. Always false for a purely visual animation.
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

/// The document's animation timeline: Stylo's animation state, the last time
/// it was sampled at, and whether anything is still moving.
#[derive(Default)]
pub(crate) struct AnimationDriver {
    sets: DocumentAnimationSet,
    now: f64,
    active: bool,
    /// Ancestors marked with the animation-only dirty-descendants bit by the
    /// last tick, kept so the same tick can clear exactly what it set. Stylo
    /// only ever sets that bit on ancestors of a hinted element, so this is a
    /// superset of what the traversal touches.
    marked: Vec<NodeId>,
    /// Elements currently carrying the `may_have_animations` bit, so the bit
    /// can be cleared again when their last animation goes away.
    flagged: Vec<NodeId>,
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

    /// The time the animations were last sampled at, in seconds.
    pub(crate) const fn now(&self) -> f64 {
        self.now
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.active
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
        self.held.retain(|(id, _)| !ids.contains(id));
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
        // guarantees a style flush follows to notice.
        self.active = sets
            .values()
            .any(stylo::servo::animation::ElementAnimationSet::needs_animation_ticks);
    }
}

impl<T: Sync> Document<T> {
    /// Whether any animation or transition still needs frames.
    #[must_use]
    pub fn has_active_animations(&self) -> bool {
        self.animations().is_active()
    }

    /// Advances every live animation and transition to `now` — seconds on a
    /// monotonic timeline whose epoch is the caller's choice — and re-cascades
    /// the elements that moved.
    ///
    /// Safe to call every frame: with nothing animating it reads one bool.
    /// Time never runs backwards; a `now` behind the last sample is clamped
    /// forward rather than rewinding the timeline.
    pub fn advance_animations(&mut self, now: f64) -> AnimationTick {
        if !self.animations().is_active() {
            self.animations_mut().now = now;
            return AnimationTick::default();
        }
        let now = now.max(self.animations().now);
        self.animations_mut().now = now;

        let hinted = self.step_animation_states(now);
        if hinted.is_empty() {
            self.sync_animation_state();
            return AnimationTick::default();
        }

        let root = self.hint_animated_elements(&hinted);
        let mut tick = self.recascade_animated_elements(root);
        // The traversal runs `process_animations` again, which prunes finished
        // animations, so what the timeline owns is only settled afterwards.
        self.sync_animation_state();
        tick.needs_next_frame = self.animations().is_active();
        if tick.restyled > 0 {
            self.note_visual_mutation();
        }
        tick
    }

    /// Re-reads Stylo's animation map: which elements own animation state,
    /// and whether anything still needs frames.
    ///
    /// Both the style flush and the tick mutate the map through Stylo, which
    /// reports neither, so the bookkeeping is rebuilt from the map itself.
    pub(crate) fn sync_animation_state(&mut self) {
        let handle = self.animations().context_handle();
        let mut held = std::mem::take(&mut self.animations_mut().held);
        let mut animated = Vec::new();
        let mut cancelled = Vec::new();
        let mut active = false;
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
                active |= set.needs_animation_ticks();
                if !set.is_empty() {
                    animated.push(id);
                }
            }
            sets.retain(|_, set| !set.is_empty());
        }
        self.animations_mut().held = held;

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
        self.animations_mut().active = active;

        if !cancelled.is_empty() {
            self.recascade_cancelled_animations(&cancelled);
        }
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
    fn recascade_cancelled_animations(&mut self, cancelled: &[NodeId]) {
        let root = self.hint_animated_elements(cancelled);
        if self.recascade_animated_elements(root).restyled > 0 {
            self.note_visual_mutation();
        }
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
        sets: &mut rustc_hash::FxHashMap<AnimationSetKey, ElementAnimationSet>,
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
    /// `Animation::update_from_other`, and `iterate_if_necessary` refuses to
    /// advance an animation that is still `Pending`, so both the start
    /// promotion and the iteration loop belong to the driver. The loop matters
    /// for a frame that stalled across several iterations.
    fn step_animation_states(&mut self, now: f64) -> Vec<NodeId> {
        let handle = self.animations().context_handle();
        let arenas = self.arenas();
        let mut hinted = Vec::new();
        let mut held = Vec::new();

        let mut sets = handle.sets.write();
        sets.retain(|key, set| {
            let Some(id) = arenas.id_at_arena_key(key.node.0) else {
                // The element was freed without a lifecycle hook reaching us.
                return false;
            };
            let mut moved = false;
            for animation in &mut set.animations {
                if animation.state == AnimationState::Pending && animation.started_at <= now {
                    animation.state = AnimationState::Running;
                    moved = true;
                }
                while animation.iterate_if_necessary(now) {
                    moved = true;
                }
                if animation.state == AnimationState::Running && animation.has_ended(now) {
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
            for transition in &mut set.transitions {
                if transition.state == AnimationState::Pending && transition.start_time <= now {
                    transition.state = AnimationState::Running;
                    moved = true;
                }
                if transition.state == AnimationState::Running && transition.has_ended(now) {
                    transition.state = AnimationState::Finished;
                    moved = true;
                }
            }
            set.clear_canceled_animations();
            if set.is_empty() {
                return false;
            }
            if set.needs_animation_ticks() || moved {
                hinted.push(id);
            }
            true
        });
        drop(sets);
        self.animations_mut().held.append(&mut held);
        hinted
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
            let traversal = RecalcStyle::new(shared);
            let token = <RecalcStyle<'_> as stylo::traversal::DomTraversal<&Node<T>>>::pre_traverse(
                root_ref,
                traversal.shared(),
            );
            if token.should_traverse() {
                let _thread_state = LayoutThreadStateGuard::enter();
                // Sequential: an animation touches a handful of elements, and
                // on Wasm worker zero of the Stylo pool is the Lynx main
                // thread, which is not this one.
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
                    let mut refreshed = None;
                    let harvested = node.stylo_data_mut().and_then(|wrapper| {
                        let mut data = wrapper.borrow_mut();
                        refreshed.clone_from(&data.styles.primary);
                        let damage = data.damage;
                        data.clear_restyle_flags_and_damage();
                        (!damage.is_empty()).then(|| StyleDamage::from(damage))
                    });
                    let refresh = node.refresh_layout_style(refreshed);
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
