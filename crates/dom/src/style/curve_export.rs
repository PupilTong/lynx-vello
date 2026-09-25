//! Exports one element's animation state as a curve the painter samples.
//!
//! The export is the element's whole stylo set, all or nothing: every
//! non-canceled animation in the set's order, cloned into the curve (see
//! [`crate::visual::curves`]), which samples them with stylo's own code.
//! The curve is exact whenever, per exported property, either nothing
//! contributed at the commit instant or every instant inside its domain
//! keeps a contribution. Both hold by construction: an animation starts
//! contributing only when a pending one starts, before which it contributed
//! nothing, and stops only when it ends without a forwards fill, at
//! `expires_at`, where the frame hands the element back to the main thread.
//! The export refuses — and the element keeps animating through per-frame
//! `BeginFrame` commits — when the painter could not reproduce the main
//! thread's cascade over the domain:
//!
//! - an animation animates a property other than `opacity` or `transform`;
//! - author `!important` on an exported property outranks the animations origin, so the main thread
//!   holds it still;
//! - a pending animation the driver has not anchored yet: its next tick moves its start;
//! - the element was frozen when the last tick ended: the next tick carries its start times;
//! - a pending or running transition: while a curve covers the element the main thread gets no
//!   ticks, so a transition that a later restyle retargets or reverses reads its progress at a
//!   stale instant and jumps. Admitting them needs the main thread's clock synced at that restyle.
//!
//! The geometry refusals are the builder's (`TransformTrack::new`).

use stylo::dom::OpaqueNode;
use stylo::properties::{LonghandId, PropertyDeclarationId};
use stylo::servo::animation::{Animation, AnimationSetKey, AnimationState};
use stylo::shared_lock::StylesheetGuards;

use crate::tree::document::Document;
use crate::tree::node::Node;
use crate::visual::curves::CompositeCurve;

/// One element's exportable animation state, minus the geometry only the
/// paint-order builder knows.
pub(crate) struct ExportedComposite {
    /// `curve.transform` starts `None`; the builder attaches it when
    /// `transform` holds.
    pub(crate) curve: CompositeCurve,
    /// Whether an animation animates `opacity`.
    pub(crate) opacity: bool,
    /// Whether an animation animates `transform`.
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
        let ended = |state: &AnimationState| {
            matches!(state, AnimationState::Canceled | AnimationState::Finished)
        };
        if set
            .transitions
            .iter()
            .any(|transition| !ended(&transition.state))
        {
            return None;
        }
        if driver.carries(node.id())
            && set.animations.iter().any(|animation| {
                matches!(
                    animation.state,
                    AnimationState::Pending | AnimationState::Running
                )
            })
        {
            return None;
        }

        let (mut opacity, mut transform) = (false, false);
        let mut animations = Vec::with_capacity(set.animations.len());
        for animation in &set.animations {
            match animation.state {
                AnimationState::Canceled => continue,
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
        if !opacity && !transform {
            return None;
        }

        let guard = self.style_engine().shared_lock().read();
        let (overriding, _) = style
            .rules()
            .get_properties_overriding_animations(&StylesheetGuards::same(&guard));
        if (opacity && overriding.contains(LonghandId::Opacity))
            || (transform && overriding.contains(LonghandId::Transform))
        {
            return None;
        }

        let expires_at = animations
            .iter()
            .filter_map(Animation::expires_at)
            .reduce(f64::min);
        Some(ExportedComposite {
            curve: CompositeCurve {
                animations: animations.into_boxed_slice(),
                expires_at,
                transform: None,
            },
            opacity,
            transform,
        })
    }
}
