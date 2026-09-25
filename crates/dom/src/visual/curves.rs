//! Exported composite-animation curves, sampled at compose time.
//!
//! A [`CompositeCurve`] is a clone of one element's stylo animation state:
//! every non-canceled `Animation` in its set's order and every live
//! `Transition`, all on `opacity` or `transform`. Sampling runs stylo's own
//! code — `Animation::progress_at` and `sample_at`, the halves of the main
//! thread's cascade, and `Transition::calculate_value` — so there is no
//! second interpolation here, and servo's deviations come along: the
//! transitions are inserted after the animations, so a transition wins its
//! property as the `Transitions` origin does. The
//! sampled values are bit-equal to the next main-thread cascade's at the
//! same instant when both sides iterate from the same animation state; start
//! times the main thread accumulates over several ticks can differ from the
//! sampler's one step in the last bit for durations that are not binary
//! fractions.
//!
//! The transform delta: content was baked at the committed world `W_c`, so
//! composition applies `planar(W(t)) · planar(W_c)⁻¹`, where `W(t)` folds the
//! sampled list through the builder's own f32 fold
//! ([`ContextMatrix::with_list`]) onto the parent's committed world. So the
//! element's world matches a commit's bit for bit relative to its parent's
//! committed world, the delta is the identity to f64 rounding at the commit
//! instant, and composed geometry agrees with a commit to f32 rounding.

use euclid::default::Transform3D;
use stylo::properties::animated_properties::{AnimationValue, AnimationValueMap};
use stylo::properties::longhands::animation_direction::computed_value::single_value::T as AnimationDirection;
use stylo::properties::{LonghandId, OwnedPropertyDeclarationId, PropertyDeclarationId};
use stylo::servo::animation::{Animation, Transition};
use stylo::values::computed::transform::Transform as ComputedTransform;

use super::reach::{Interval, Reach, Segment, eased_range};
use super::transform::{ContextMatrix, planar};
use crate::vello::kurbo::Affine;

const OPACITY: OwnedPropertyDeclarationId =
    OwnedPropertyDeclarationId::Longhand(LonghandId::Opacity);
const TRANSFORM: OwnedPropertyDeclarationId =
    OwnedPropertyDeclarationId::Longhand(LonghandId::Transform);

/// One element's exported animation state.
#[derive(Debug, Clone)]
pub(crate) struct CompositeCurve {
    /// The set's animations in its order, canceled ones dropped: the order
    /// the cascade inserts them in, a later one winning a property.
    pub(crate) animations: Box<[Animation]>,
    /// The set's pending and running transitions in its order.
    pub(crate) transitions: Box<[Transition]>,
    /// The timeline second the first animation or transition ends at: past
    /// it its contribution can be replaced by the base value, which only a
    /// commit knows. `None` when nothing ends.
    pub(crate) expires_at: Option<f64>,
    /// Present when an animation animates `transform`.
    pub(crate) transform: Option<TransformTrack>,
}

/// The constant factors the transform delta folds a sampled list with.
#[derive(Debug, Clone)]
pub(crate) struct TransformTrack {
    context: ContextMatrix,
    parent_world: Transform3D<f32>,
    /// `planar(W_c)⁻¹`: the committed world the frame was baked at, undone.
    committed_inverse: Affine,
    /// Where the delta can carry content over the curve's domain.
    pub(crate) reach: Reach,
}

impl TransformTrack {
    /// The track of `curve` on an element whose world folds as `context`
    /// then `parent_world`, committed at `world` with transform list
    /// `committed`. `None` where composition, a 2D affine, cannot follow: a
    /// committed world that is not 2D or not invertible, or a keyframe value
    /// some interpolation takes out of the plane.
    pub(crate) fn new(
        curve: &CompositeCurve,
        context: ContextMatrix,
        parent_world: Transform3D<f32>,
        world: &Transform3D<f32>,
        committed: &ComputedTransform,
    ) -> Option<Self> {
        let ends = curve.transition_ends();
        let transitions = ends.iter().map(|(from, to)| Segment {
            from,
            to,
            // The timing function is private to the fork's `PropertyAnimation`,
            // so its eased range is unknown and the reach unbounded.
            eased: None,
        });
        if !world.is_2d()
            || curve
                .transform_segments()
                .chain(transitions.clone())
                .any(|segment| context.projective(segment.from) || context.projective(segment.to))
        {
            return None;
        }
        let world = planar(world);
        let committed_matrix = planar(&context.list_matrix(committed).0);
        if committed_matrix.determinant().abs() < 1e-9 || world.determinant().abs() < 1e-9 {
            return None;
        }
        // `W = pre · Lc · origin⁻¹`, every factor of `pre` constant.
        let pre = world * context.origin() * committed_matrix.inverse();
        let reach = Reach::of(
            curve.transform_segments().chain(transitions),
            committed,
            context.reference_size(),
            pre,
            committed_matrix,
        );
        Some(Self {
            context,
            parent_world,
            committed_inverse: world.inverse(),
            reach,
        })
    }

    /// The element's world with `list` as its transform.
    pub(crate) fn world(&self, list: &ComputedTransform) -> Transform3D<f32> {
        self.context.with_list(list).then(&self.parent_world)
    }

    fn delta(&self, list: &ComputedTransform) -> Affine {
        planar(&self.world(list)) * self.committed_inverse
    }
}

/// What one sample answers: the CSS-px delta against the committed bake and
/// the opacity replacing the committed one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CurveSample {
    pub(crate) delta: Affine,
    pub(crate) alpha: Option<f32>,
}

impl CompositeCurve {
    /// Samples the curve at timeline second `now`, with `values` as scratch.
    /// A property no animation contributes keeps its committed value:
    /// identity delta, `None` alpha.
    pub(crate) fn sample(&self, now: f64, values: &mut AnimationValueMap) -> CurveSample {
        self.values_at(now, values);
        let alpha = match values.get(&OPACITY) {
            // Paint clamps the committed opacity the same way.
            Some(AnimationValue::Opacity(opacity)) => Some(opacity.clamp(0.0, 1.0)),
            _ => None,
        };
        let delta = match (&self.transform, values.get(&TRANSFORM)) {
            (Some(track), Some(AnimationValue::Transform(list))) => track.delta(list),
            _ => Affine::IDENTITY,
        };
        CurveSample { delta, alpha }
    }

    /// Fills `values` with what the cascade at `now` takes from the
    /// animations, in their order, then from the transitions over them.
    pub(crate) fn values_at(&self, now: f64, values: &mut AnimationValueMap) {
        // Past its domain the curve holds the domain's last instant until the
        // hand-back commit is adopted.
        let now = self.expires_at.map_or(now, |end| now.min(end.next_down()));
        values.clear();
        for animation in &self.animations {
            if let Some(at) = animation.progress_at(now) {
                animation.sample_at(at, values);
            }
        }
        for transition in &self.transitions {
            let value = transition.calculate_value(now);
            values.insert(value.id().to_owned(), value);
        }
    }

    /// Each `transform` transition's value at its start and at its end.
    ///
    /// Sampled, because the fork keeps a transition's own `from`, `to` and
    /// timing function private: at the start the eased progress is 0 for
    /// every timing function but a jump-start `steps()`, and at the end it is
    /// 1 for all of them.
    fn transition_ends(&self) -> Vec<(ComputedTransform, ComputedTransform)> {
        let list = |value: AnimationValue| match value {
            AnimationValue::Transform(list) => list,
            _ => unreachable!("a transform transition samples a transform list"),
        };
        self.transitions
            .iter()
            .filter(|transition| {
                transition.property_animation.property_id()
                    == PropertyDeclarationId::Longhand(LonghandId::Transform)
            })
            .map(|transition| {
                let end = transition.start_time + transition.property_animation.duration;
                (
                    list(transition.calculate_value(transition.start_time)),
                    list(transition.calculate_value(end)),
                )
            })
            .collect()
    }

    /// Every segment an animation interpolates `transform` over, with the
    /// eased progress it samples there: eased by the lower keyframe's
    /// function running forward and by the upper's over flipped progress
    /// running reversed, as stylo eases them. Iterations and fill only pick
    /// progress in `[0, 1]`.
    fn transform_segments(&self) -> impl Iterator<Item = Segment<'_>> {
        let transform = PropertyDeclarationId::Longhand(LonghandId::Transform);
        self.animations.iter().flat_map(move |animation| {
            let index = animation
                .animating_properties()
                .find_map(|(index, property)| (property == transform).then_some(index));
            index
                .into_iter()
                .flat_map(|index| animation.keyframe_segments(index))
                .map(move |segment| {
                    let (AnimationValue::Transform(from), AnimationValue::Transform(to)) =
                        (segment.from.value, segment.to.value)
                    else {
                        unreachable!("a transform keyframe holds a transform list");
                    };
                    let forward = eased_range(segment.from.timing_function);
                    let reversed = eased_range(segment.to.timing_function).map(Interval::flipped);
                    let eased = match animation.direction {
                        AnimationDirection::Normal => forward,
                        AnimationDirection::Reverse => reversed,
                        AnimationDirection::Alternate | AnimationDirection::AlternateReverse => {
                            forward.zip(reversed).map(|(a, b)| a.union(b))
                        }
                    };
                    Segment { from, to, eased }
                })
        })
    }
}

#[cfg(test)]
impl CompositeCurve {
    /// A curve with no animations, so it samples the committed values: the
    /// shape the compose-program tests need.
    pub(crate) fn inert(transform: Option<TransformTrack>) -> Self {
        Self {
            animations: Box::new([]),
            transitions: Box::new([]),
            expires_at: None,
            transform,
        }
    }

    /// Whether an animation or a transition animates `property`.
    pub(crate) fn animates(&self, property: LonghandId) -> bool {
        let property = PropertyDeclarationId::Longhand(property);
        self.animations.iter().any(|animation| {
            animation
                .animating_properties()
                .any(|(_, animated)| animated == property)
        }) || self
            .transitions
            .iter()
            .any(|transition| transition.property_animation.property_id() == property)
    }
}

#[cfg(test)]
impl TransformTrack {
    /// A track on an element at the root with a zero-sized box, whose reach
    /// is `reach`.
    pub(crate) fn with_reach(reach: Reach) -> Self {
        Self {
            context: ContextMatrix::identity(),
            parent_world: Transform3D::identity(),
            committed_inverse: Affine::IDENTITY,
            reach,
        }
    }
}
