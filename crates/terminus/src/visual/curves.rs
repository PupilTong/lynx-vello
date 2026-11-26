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
//! compose time from the live offset, with no commit and no clock. On an
//! `overscroll-behavior: circular` axis a scroll progress timeline first
//! takes an offset through the seam at its representative nearer the range
//! ([`ScrollSlot::unwrap`](super::ScrollSlot::unwrap)), as the main thread
//! does; a view progress timeline reads the offset as it stands in the
//! period, unclamped, and the seam copy of the container's content samples
//! it again one period back ([`ProgressKind`],
//! [`SeamPass`](super::space::SeamPass)).

use euclid::default::Transform3D;
use stylo::properties::animated_properties::{AnimationValue, AnimationValueMap};
use stylo::properties::longhands::animation_direction::computed_value::single_value::T as AnimationDirection;
use stylo::properties::{LonghandId, OwnedPropertyDeclarationId, PropertyDeclarationId};
use stylo::servo::animation::{Animation, Transition};
use stylo::values::computed::transform::Transform as ComputedTransform;

use super::ScrollOffsets;
use super::reach::{Interval, Reach, Segment, eased_range};
use super::transform::{ContextMatrix, planar};
use crate::kurbo::Affine;
use crate::style::timeline::{Axis, ProgressKind, ProgressTiming, iteration_progress};
use crate::tree::document::NodeId;

const OPACITY: OwnedPropertyDeclarationId =
    OwnedPropertyDeclarationId::Longhand(LonghandId::Opacity);
const TRANSFORM: OwnedPropertyDeclarationId =
    OwnedPropertyDeclarationId::Longhand(LonghandId::Transform);

#[derive(Debug, Clone)]
pub(crate) struct CompositeCurve {
    pub(crate) animations: Box<[(Animation, Timeline)]>,
    pub(crate) transitions: Box<[Transition]>,
    pub(crate) committed_at: f64,
    pub(crate) expires_at: Option<f64>,
    pub(crate) transform: Option<TransformTrack>,
}

#[derive(Debug, Clone)]
pub(crate) enum Timeline {
    Clock,
    Scroll(ScrollTimeline),
}

#[derive(Debug, Clone)]
pub(crate) struct ScrollTimeline {
    pub(crate) source: NodeId,
    pub(crate) slot: Option<u32>,
    pub(crate) axis: Axis,
    pub(crate) timing: ProgressTiming,
    pub(crate) kind: ProgressKind,
}

impl ScrollTimeline {
    pub(crate) fn views(&self, slot: u32) -> bool {
        self.kind == ProgressKind::View && self.slot == Some(slot)
    }

    fn progress(
        &self,
        slot: u32,
        offsets: &ScrollOffsets<'_>,
    ) -> Option<stylo::servo::animation::AnimationProgress> {
        let scroll_slot = &offsets.slots[slot as usize];
        let offset = offsets.of(slot);
        let (offset, clamped) = match self.kind {
            ProgressKind::Scroll => (scroll_slot.unwrap(offset), true),
            ProgressKind::View => {
                let (x, y) = scroll_slot.wrap_period();
                let wraps = match self.axis {
                    Axis::X => x.is_some(),
                    Axis::Y => y.is_some(),
                };
                (offset, !wraps)
            }
        };
        let offset = self.axis.of_vector(offset);
        let offset = if clamped {
            offset.max(0.0).min(self.timing.limit)
        } else {
            offset
        };
        iteration_progress(&self.timing, offset)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TransformTrack {
    context: ContextMatrix,
    parent_world: Transform3D<f32>,
    committed_inverse: Affine,
    pub(crate) reach: Reach,
}

impl TransformTrack {
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

    pub(crate) fn world(&self, list: &ComputedTransform) -> Transform3D<f32> {
        self.context.with_list(list).then(&self.parent_world)
    }

    fn delta(&self, list: &ComputedTransform) -> Affine {
        planar(&self.world(list)) * self.committed_inverse
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CurveSample {
    pub(crate) delta: Affine,
    pub(crate) alpha: Option<f32>,
}

impl CompositeCurve {
    pub(crate) fn sample(
        &self,
        now: Option<f64>,
        offsets: &ScrollOffsets<'_>,
        values: &mut AnimationValueMap,
    ) -> CurveSample {
        self.values_at(now, offsets, values);
        let alpha = match values.get(&OPACITY) {
            Some(AnimationValue::Opacity(opacity)) => Some(opacity.clamp(0.0, 1.0)),
            _ => None,
        };
        let delta = match (&self.transform, values.get(&TRANSFORM)) {
            (Some(track), Some(AnimationValue::Transform(list))) => track.delta(list),
            _ => Affine::IDENTITY,
        };
        CurveSample { delta, alpha }
    }

    pub(crate) fn views(&self, slot: u32) -> bool {
        self.animations
            .iter()
            .any(|(_, timeline)| matches!(timeline, Timeline::Scroll(scroll) if scroll.views(slot)))
    }

    pub(crate) fn values_at(
        &self,
        now: Option<f64>,
        offsets: &ScrollOffsets<'_>,
        values: &mut AnimationValueMap,
    ) {
        let now = now.unwrap_or(self.committed_at);
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

    pub(crate) fn reads_clock(&self) -> bool {
        !self.transitions.is_empty()
            || self
                .animations
                .iter()
                .any(|(animation, _)| !animation.is_progress_driven())
    }

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
    pub(crate) fn inert(transform: Option<TransformTrack>) -> Self {
        Self {
            animations: Box::new([]),
            transitions: Box::new([]),
            committed_at: 0.0,
            expires_at: None,
            transform,
        }
    }

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
    pub(crate) fn with_reach(reach: Reach) -> Self {
        Self {
            context: ContextMatrix::identity(),
            parent_world: Transform3D::identity(),
            committed_inverse: Affine::IDENTITY,
            reach,
        }
    }
}
