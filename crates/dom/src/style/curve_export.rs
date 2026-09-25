//! Exports one element's animation state as a curve the painter samples.
//!
//! The export is the element's whole stylo set, all or nothing: every
//! non-canceled animation in the set's order and every pending or running
//! transition, cloned into the curve (see [`crate::visual::curves`]), which
//! samples them with stylo's own code. The curve is exact whenever, per
//! exported property, either nothing contributed at the commit instant or
//! every instant inside its domain keeps a contribution. Both hold by
//! construction: an animation starts contributing only when a pending one
//! starts, before which it contributed nothing, and stops only when it ends
//! without a forwards fill, at `expires_at`, where the frame hands the
//! element back to the main thread; a transition contributes from its
//! creation to its end, its delay included.
//!
//! A transition is exportable because every job on the main thread starts
//! with [`Document::sync_animation_clock`] at the painter's clock: a restyle
//! that retargets or reverses one while a curve covers the element reads
//! its progress at the current instant, not at the last tick.
//!
//! The export refuses — and the element keeps animating through per-frame
//! main-thread ticks — when the painter could not reproduce the main
//! thread's cascade over the domain:
//!
//! - an animation animates a property other than `opacity` or `transform`;
//! - a transition on any other property;
//! - author `!important` on a property only animations drive (a transition outranks it);
//! - a pending animation or transition the driver has not anchored yet: its next tick moves its
//!   start;
//! - the element was frozen when the last tick ended: the next tick carries its start times;
//! - a progress-driven animation (scroll-animations-1): no curve carries a scroll timeline yet, and
//!   as a clock curve it would hold the committed sample and recompose every frame. It re-cascades
//!   on the main thread when its scroll container moves instead of ticking.
//!
//! The geometry refusals are the builder's (`TransformTrack::new`).

use stylo::dom::OpaqueNode;
use stylo::properties::{LonghandId, PropertyDeclarationId};
use stylo::servo::animation::{Animation, AnimationSetKey, AnimationState, Transition};
use stylo::shared_lock::StylesheetGuards;

use crate::style::animation::AnimationDriver;
use crate::tree::document::Document;
use crate::tree::node::Node;
use crate::visual::curves::CompositeCurve;

/// One element's exportable animation state, minus the geometry only the
/// paint-order builder knows.
pub(crate) struct ExportedComposite {
    /// `curve.transform` starts `None`; the builder attaches it when
    /// `transform` holds.
    pub(crate) curve: CompositeCurve,
    /// Whether an animation or a transition animates `opacity`.
    pub(crate) opacity: bool,
    /// Whether an animation or a transition animates `transform`.
    pub(crate) transform: bool,
}

impl<T: Sync> Document<T> {
    /// The exportable animation state of `node`, if the whole of it
    /// exports; see the module documentation for what refuses.
    pub(crate) fn composite_export(&self, node: &Node<T>) -> Option<ExportedComposite> {
        if !node.may_have_animations() {
            return None;
        }
        let style = self.paint_style(node.id())?;
        let driver = self.animations();
        let handle = driver.context_handle();
        let sets = handle.sets.read();
        let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(node.id().arena_key()));
        let set = sets.get(&key)?;
        let live = |state: &AnimationState| {
            matches!(state, AnimationState::Pending | AnimationState::Running)
        };
        if driver.carries(node.id())
            && (set
                .animations
                .iter()
                .any(|animation| live(&animation.state))
                || set
                    .transitions
                    .iter()
                    .any(|transition| live(&transition.state)))
        {
            return None;
        }

        let (mut opacity, mut transform) = (false, false);
        let mut animations = Vec::with_capacity(set.animations.len());
        for animation in &set.animations {
            match animation.state {
                AnimationState::Canceled => continue,
                _ if animation.is_progress_driven() => return None,
                AnimationState::Pending if !driver.keyframes_anchored(&key, &animation.name) => {
                    return None;
                }
                _ => {}
            }
            for (_, property) in animation.animating_properties() {
                match property {
                    PropertyDeclarationId::Longhand(LonghandId::Opacity) => opacity = true,
                    PropertyDeclarationId::Longhand(LonghandId::Transform) => transform = true,
                    _ => return None,
                }
            }
            let mut animation = animation.clone();
            // The driver promotes a pending animation on the first tick at or
            // past its start, then iterates it. Stylo samples a pending and a
            // running animation alike before the start, so a running copy
            // samples what the main thread does at every instant.
            if animation.state == AnimationState::Pending {
                animation.state = AnimationState::Running;
            }
            animations.push(animation);
        }
        let transitions = exported_transitions(driver, &key, &set.transitions)?;
        let transitioned = |longhand: LonghandId| {
            transitions.iter().any(|transition| {
                transition.property_animation.property_id()
                    == PropertyDeclarationId::Longhand(longhand)
            })
        };
        // The `Transitions` origin outranks author `!important`, so only a
        // property animations alone drive can be held still by one.
        let (animated_opacity, animated_transform) = (opacity, transform);
        opacity |= transitioned(LonghandId::Opacity);
        transform |= transitioned(LonghandId::Transform);
        if !opacity && !transform {
            return None;
        }

        let guard = self.style_engine().shared_lock().read();
        let (overriding, _) = style
            .rules()
            .get_properties_overriding_animations(&StylesheetGuards::same(&guard));
        let held = |animated: bool, longhand: LonghandId| {
            animated && overriding.contains(longhand) && !transitioned(longhand)
        };
        if held(animated_opacity, LonghandId::Opacity)
            || held(animated_transform, LonghandId::Transform)
        {
            return None;
        }

        let expires_at = animations
            .iter()
            .filter_map(Animation::expires_at)
            .chain(transitions.iter().map(transition_end))
            .reduce(f64::min);
        Some(ExportedComposite {
            curve: CompositeCurve {
                animations: animations.into_boxed_slice(),
                transitions: transitions.into_boxed_slice(),
                expires_at,
                transform: None,
            },
            opacity,
            transform,
        })
    }
}

/// The pending and running ones of `transitions`, in order; `None` when one
/// animates a property other than `opacity` or `transform`, or is pending and
/// not anchored yet.
fn exported_transitions(
    driver: &AnimationDriver,
    key: &AnimationSetKey,
    transitions: &[Transition],
) -> Option<Vec<Transition>> {
    let mut exported = Vec::new();
    for transition in transitions {
        let pending = match transition.state {
            AnimationState::Pending => true,
            AnimationState::Running => false,
            _ => continue,
        };
        let property = transition.property_animation.property_id();
        if pending && !driver.transition_anchored(key, property) {
            return None;
        }
        if !matches!(
            property,
            PropertyDeclarationId::Longhand(LonghandId::Opacity | LonghandId::Transform)
        ) {
            return None;
        }
        exported.push(transition.clone());
    }
    Some(exported)
}

/// The timeline second `transition` ends at, where the cascade drops it.
fn transition_end(transition: &Transition) -> f64 {
    transition.start_time + transition.property_animation.duration
}
