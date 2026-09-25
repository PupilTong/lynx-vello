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
//!
//! An animation on a scroll progress timeline (scroll-animations-1) reads
//! its progress from its source's slot instead of the clock: the offset
//! compose puts that slot at, clamped to the scroll range and unsnapped as
//! the cascade reads it, through the same [`iteration_progress`] the main
//! thread writes `Animation::timeline_sample` with. So it is sampled at
//! compose time from the live offset, with no commit and no clock.

use euclid::default::Transform3D;
use stylo::properties::animated_properties::{AnimationValue, AnimationValueMap};
use stylo::properties::longhands::animation_direction::computed_value::single_value::T as AnimationDirection;
use stylo::properties::{LonghandId, OwnedPropertyDeclarationId, PropertyDeclarationId};
use stylo::servo::animation::{Animation, Transition};
use stylo::values::computed::transform::Transform as ComputedTransform;

use super::ScrollOffsets;
use super::reach::{Interval, Reach, Segment, eased_range};
use super::transform::{ContextMatrix, planar};
use crate::NodeId;
use crate::style::timeline::{Axis, ProgressTiming, iteration_progress};
use crate::vello::kurbo::Affine;

const OPACITY: OwnedPropertyDeclarationId =
    OwnedPropertyDeclarationId::Longhand(LonghandId::Opacity);
const TRANSFORM: OwnedPropertyDeclarationId =
    OwnedPropertyDeclarationId::Longhand(LonghandId::Transform);

/// One element's exported animation state.
#[derive(Debug, Clone)]
pub(crate) struct CompositeCurve {
    /// The set's animations in its order, canceled ones dropped: the order
    /// the cascade inserts them in, a later one winning a property. Each
    /// with the timeline it reads its progress from.
    pub(crate) animations: Box<[(Animation, Timeline)]>,
    /// The set's pending and running transitions in its order.
    pub(crate) transitions: Box<[Transition]>,
    /// The timeline second the curve was committed at: a sample with no
    /// clock reading samples the document timeline here, which is the
    /// committed cascade's instant.
    pub(crate) committed_at: f64,
    /// The timeline second the first animation or transition ends at: past
    /// it its contribution can be replaced by the base value, which only a
    /// commit knows. `None` when nothing ends.
    pub(crate) expires_at: Option<f64>,
    /// Present when an animation animates `transform`.
    pub(crate) transform: Option<TransformTrack>,
}

/// Where one exported animation reads its progress.
#[derive(Debug, Clone)]
pub(crate) enum Timeline {
    /// `Animation::progress_at`: the document timeline at the instant
    /// sampled; for a progress-driven animation, its cloned
    /// `timeline_sample` — held while paused, `None` on an inactive timeline.
    Clock,
    /// A running animation on an active scroll progress timeline.
    Scroll(ScrollTimeline),
}

/// A running animation's scroll progress timeline, as the commit bound it.
#[derive(Debug, Clone)]
pub(crate) struct ScrollTimeline {
    /// The scroll container whose offset drives it.
    pub(crate) source: NodeId,
    /// `source`'s scroll slot in this frame, bound after the build's walk
    /// (the element's own slot, and a later-painted source's, do not exist
    /// yet when the curve is built). `None` when the frame has none: nothing
    /// moves the source between commits then, and the animation holds its
    /// cloned sample.
    pub(crate) slot: Option<u32>,
    pub(crate) axis: Axis,
    pub(crate) timing: ProgressTiming,
}

impl ScrollTimeline {
    /// Where the animation stands with `slot` at `offsets`: the offset along
    /// `axis`, clamped to the scroll range as the document clamps it (a
    /// `contain-bounce` stretch is no scroll offset) and unsnapped.
    fn progress(
        &self,
        slot: u32,
        offsets: &ScrollOffsets<'_>,
    ) -> Option<stylo::servo::animation::AnimationProgress> {
        let offset = self
            .axis
            .of_vector(offsets.of(slot))
            .max(0.0)
            .min(self.timing.limit);
        iteration_progress(&self.timing, offset)
    }
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
        if !world.is_2d()
            || curve
                .transform_segments()
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
            curve.transform_segments(),
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
    /// Samples the curve at timeline second `now` — the committed instant
    /// when `None` — with each scroll slot at `offsets`, and `values` as
    /// scratch. A property no animation contributes keeps its committed
    /// value: identity delta, `None` alpha.
    pub(crate) fn sample(
        &self,
        now: Option<f64>,
        offsets: &ScrollOffsets<'_>,
        values: &mut AnimationValueMap,
    ) -> CurveSample {
        self.values_at(now, offsets, values);
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

    /// Fills `values` with what the cascade at `now` (the committed instant
    /// when `None`) and `offsets` takes from the animations, in their order,
    /// then from the transitions over them.
    pub(crate) fn values_at(
        &self,
        now: Option<f64>,
        offsets: &ScrollOffsets<'_>,
        values: &mut AnimationValueMap,
    ) {
        let now = now.unwrap_or(self.committed_at);
        // Past its domain the curve holds the domain's last instant until the
        // hand-back commit is adopted.
        let now = self.expires_at.map_or(now, |end| now.min(end.next_down()));
        values.clear();
        for (animation, timeline) in &self.animations {
            let at = match timeline {
                Timeline::Scroll(
                    scroll @ ScrollTimeline {
                        slot: Some(slot), ..
                    },
                ) => scroll.progress(*slot, offsets),
                _ => animation.progress_at(now),
            };
            if let Some(at) = at {
                animation.sample_at(at, values);
            }
        }
        for transition in &self.transitions {
            let value = transition.calculate_value(now);
            values.insert(value.id().to_owned(), value);
        }
    }

    /// Every segment an animation or a transition interpolates `transform`
    /// over, with the eased progress it samples there. An animation's
    /// segment eases by the lower keyframe's function running forward and by
    /// the upper's over flipped progress running reversed, as stylo eases
    /// them, in the direction the sample runs; iterations, fill and a scroll
    /// timeline only pick progress in `[0, 1]`. A transition runs once forward from its `from` to
    /// its `to`.
    fn transform_segments(&self) -> impl Iterator<Item = Segment<'_>> {
        fn list(value: &AnimationValue) -> &ComputedTransform {
            let AnimationValue::Transform(list) = value else {
                unreachable!("a transform transition holds transform lists");
            };
            list
        }
        let transform = PropertyDeclarationId::Longhand(LonghandId::Transform);
        let transitions = self
            .transitions
            .iter()
            .map(|transition| &transition.property_animation)
            .filter(move |animation| animation.property_id() == transform)
            .map(move |animation| Segment {
                from: list(animation.from()),
                to: list(animation.to()),
                eased: eased_range(animation.timing_function()),
            });
        let animations = self
            .animations
            .iter()
            .flat_map(move |(animation, timeline)| {
                // A progress-driven animation's direction is its binding's, read
                // from the style by name, which stylo's `return;` deviation can
                // leave apart from the clone's; a held sample's binding is gone.
                let direction = match timeline {
                    Timeline::Scroll(scroll) => Some(scroll.timing.direction),
                    Timeline::Clock if animation.is_progress_driven() => None,
                    Timeline::Clock => Some(animation.direction),
                };
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
                        let reversed =
                            eased_range(segment.to.timing_function).map(Interval::flipped);
                        let eased = match direction {
                            Some(AnimationDirection::Normal) => forward,
                            Some(AnimationDirection::Reverse) => reversed,
                            _ => forward.zip(reversed).map(|(a, b)| a.union(b)),
                        };
                        Segment { from, to, eased }
                    })
            });
        animations.chain(transitions)
    }

    /// Whether an animation or a transition reads the document timeline:
    /// the compositor then recomposes each frame at its clock reading.
    pub(crate) fn reads_clock(&self) -> bool {
        !self.transitions.is_empty()
            || self
                .animations
                .iter()
                .any(|(animation, _)| !animation.is_progress_driven())
    }

    /// Whether an animation reads a scroll slot's offset.
    pub(crate) fn reads_scroll(&self) -> bool {
        self.animations.iter().any(|(_, timeline)| {
            matches!(
                timeline,
                Timeline::Scroll(ScrollTimeline { slot: Some(_), .. })
            )
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
            committed_at: 0.0,
            expires_at: None,
            transform,
        }
    }

    /// Whether an animation or a transition animates `property`.
    pub(crate) fn animates(&self, property: LonghandId) -> bool {
        let property = PropertyDeclarationId::Longhand(property);
        self.animations.iter().any(|(animation, _)| {
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
