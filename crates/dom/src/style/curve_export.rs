//! Exports one element's animation state as a curve the painter samples.
//!
//! The export is the element's whole stylo set, all or nothing: every
//! non-canceled animation in the set's order and every pending or running
//! transition, cloned into the curve (see [`crate::visual::curves`]), which
//! samples them with stylo's own code. A running animation on an active
//! scroll timeline takes its binding along, and the painter samples it from
//! its source's live offset.
//!
//! The curve is exact whenever, per exported property, either nothing
//! contributed at the commit instant and offsets — the committed value is
//! then the base value, which a sample without that property reproduces —
//! or every instant and offset inside its domain keeps a contribution. On
//! the document timeline that holds by construction: an animation starts
//! contributing only when a pending one starts, before which it contributed
//! nothing, and stops only when it ends without a forwards fill, at
//! `expires_at`, where the frame hands the element back to the main thread;
//! a transition contributes from its creation to its end, its delay
//! included. A scroll timeline has no hand-back — its offsets move both
//! ways — so a property contributed at the commit must keep a contributor
//! with an effect over the whole scroll range: one on the document timeline
//! contributing now, a transition, a held progress-driven sample, or a
//! scroll-driven animation with no before or after phase in that range
//! without the matching fill. The specification's examples use `both` for
//! that reason.
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
//! - a property contributed at the commit that some offset of a scroll timeline leaves without a
//!   contribution (above). Such an animation re-cascades on the main thread when its scroll
//!   container moves instead.
//!
//! The geometry refusals are the builder's (`TransformTrack::new`).

use stylo::dom::OpaqueNode;
use stylo::properties::{LonghandId, PropertyDeclarationId};
use stylo::servo::animation::{Animation, AnimationSetKey, AnimationState, Transition};
use stylo::shared_lock::StylesheetGuards;

use crate::style::animation::{AnimationDriver, animation_has_side_effects};
use crate::style::timeline::Binding;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;
use crate::visual::curves::{CompositeCurve, ScrollTimeline, Timeline};

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

        let now = driver.now();
        let Animations {
            entries: animations,
            animated: [mut opacity, mut transform],
            committed,
            mut steady,
        } = exported_animations(driver, &key, node.id(), &set.animations, now)?;
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
        // A transition contributes from its creation to its end.
        steady[0] |= transitioned(LonghandId::Opacity);
        steady[1] |= transitioned(LonghandId::Transform);
        if committed
            .into_iter()
            .zip(steady)
            .any(|(committed, steady)| committed && !steady)
        {
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
            .filter_map(|(animation, _)| animation.expires_at())
            .chain(transitions.iter().map(transition_end))
            .reduce(f64::min);
        Some(ExportedComposite {
            curve: CompositeCurve {
                animations: animations.into_boxed_slice(),
                transitions: transitions.into_boxed_slice(),
                committed_at: now,
                expires_at,
                transform: None,
            },
            opacity,
            transform,
        })
    }
}

/// One element's exportable animations, with what the base-value rule
/// reads per property, `[opacity, transform]`.
struct Animations {
    entries: Vec<(Animation, Timeline)>,
    /// Whether an animation with side effects animates the property.
    animated: [bool; 2],
    /// Whether one contributes to it at the commit instant and offsets.
    committed: [bool; 2],
    /// Whether one contributes to it at every instant and offset of the
    /// domain.
    steady: [bool; 2],
}

/// The non-canceled ones of `id`'s `animations`, in order, each with its
/// timeline; `None` when one animates a property other than `opacity` or
/// `transform`, or is pending and not anchored yet.
fn exported_animations(
    driver: &AnimationDriver,
    key: &AnimationSetKey,
    id: NodeId,
    animations: &[Animation],
    now: f64,
) -> Option<Animations> {
    let mut exported = Animations {
        entries: Vec::with_capacity(animations.len()),
        animated: [false; 2],
        committed: [false; 2],
        steady: [false; 2],
    };
    for animation in animations {
        let progress_driven = animation.is_progress_driven();
        match animation.state {
            AnimationState::Canceled => continue,
            AnimationState::Pending
                if !progress_driven && !driver.keyframes_anchored(key, &animation.name) =>
            {
                return None;
            }
            _ => {}
        }
        let mut properties = [false; 2];
        for (_, property) in animation.animating_properties() {
            match property {
                PropertyDeclarationId::Longhand(LonghandId::Opacity) => properties[0] = true,
                PropertyDeclarationId::Longhand(LonghandId::Transform) => properties[1] = true,
                _ => return None,
            }
        }
        // What the driver's `animates` bits count: a finished animation
        // with no forwards fill, or one on an inactive timeline, animates
        // nothing, though it stays in the curve to keep the set's order.
        let current = animation_has_side_effects(animation, || {
            driver.timelines.is_current(id, &animation.name)
        });
        let mut animation = animation.clone();
        // The driver promotes a pending animation on the first tick at or
        // past its start, then iterates it. Stylo samples a pending and a
        // running animation alike before the start, so a running copy
        // samples what the main thread does at every instant.
        if animation.state == AnimationState::Pending {
            animation.state = AnimationState::Running;
        }
        // A paused one holds its sample, as the main thread does.
        let timeline = match driver.timelines.binding_of(id, &animation.name) {
            Some(&Binding::Active {
                source,
                axis,
                timing,
            }) if progress_driven && animation.state == AnimationState::Running => {
                Timeline::Scroll(ScrollTimeline {
                    source,
                    slot: None,
                    axis,
                    timing,
                })
            }
            _ => Timeline::Clock,
        };
        let contributes = animation.progress_at(now).is_some();
        let keeps = match &timeline {
            Timeline::Scroll(scroll) => scroll.timing.contributes_throughout(),
            Timeline::Clock => contributes,
        };
        for (property, animates) in properties.into_iter().enumerate() {
            exported.animated[property] |= animates && current;
            exported.committed[property] |= animates && contributes;
            exported.steady[property] |= animates && keeps;
        }
        exported.entries.push((animation, timeline));
    }
    Some(exported)
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
