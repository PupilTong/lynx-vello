//! Re-expresses one element's running CSS animation as an exported curve.
//!
//! Stylo's per-keyframe computed values are private, so the exporter reads
//! the two public surfaces that together determine them: the `Animation`'s
//! timing fields, and the stylist's `@keyframes` steps — whose declaration
//! blocks it converts *literally*. Only context-free values convert: plain
//! numbers, absolute lengths, angles. Anything else — `em`, percentages,
//! `calc()`, `var()`, unmatched transform lists, a second running
//! animation, any transition — refuses the export and the element keeps
//! animating through per-frame `BeginFrame` commits. A refusal can never be
//! wrong; only an inexact conversion could be, so conversions are exact or
//! absent.
//!
//! The compositor's samples must land where the main thread's restyle would
//! land at the same instant — the refill commit's values are the same
//! curve's — so the track structure mirrors `ComputedKeyframe` generation:
//! collapsed same-percentage steps, per-step timing functions defaulting to
//! the element's `animation-timing-function`, and per-property declaring
//! keyframes. Where stylo backfills a missing `from`/`to` from the base
//! style, the exporter refuses instead: the base style at animation start
//! is not recoverable from outside.

#![expect(
    clippy::float_cmp,
    reason = "keyframe percentages collapse and validate on exact equality, \
              as stylo's own step collapse does"
)]

use stylo::dom::OpaqueNode;
use stylo::properties::longhands::animation_direction::computed_value::single_value::T as AnimationDirection;
use stylo::properties::{ComputedValues, PropertyDeclaration};
use stylo::servo::animation::{AnimationSetKey, AnimationState, KeyframesIterationState};
use stylo::shared_lock::SharedRwLockReadGuard;
use stylo::stylesheets::keyframes_rule::{KeyframesStep, KeyframesStepValue};
use stylo::values::generics::easing::{TimingFunction, TimingKeyword};
use stylo::values::generics::transform::GenericTransformOperation;
use stylo_traits::ToCss;

use crate::tree::document::Document;
use crate::tree::node::Node;
use crate::visual::curves::{
    CompositeCurve, DirectionState, Easing, Iterations, Track, TrackPoint, TransformList,
    TransformOp,
};

/// One element's exportable animation, minus the geometry only the
/// paint-order builder knows (the world matrix the transform delta needs).
pub(crate) struct ExportedComposite {
    pub(crate) curve: CompositeCurve,
    /// The transform track, waiting for the builder to attach `pre` and the
    /// committed inverse. `curve.transform` starts `None` and is filled by
    /// the builder from this.
    pub(crate) transform_track: Option<Track<TransformList>>,
    /// The committed transform as a list in the same vocabulary, so the
    /// delta's `Lc⁻¹` is built by the exact code that builds `L(t)`.
    pub(crate) committed_transform: TransformList,
}

impl<T: Sync> Document<T> {
    /// Whether `node`'s running animation moves only composite properties —
    /// the paint-order builder's cue to treat the element as a stacking
    /// context with a composited group even where its committed style would
    /// not make one.
    pub(crate) fn animates_composite_properties(&self, node: &Node<T>) -> bool {
        if !node.may_have_animations() {
            return false;
        }
        self.composite_export(node).is_some()
    }

    /// The exportable composite animation on `node`, if its whole animation
    /// state is exportable; see the module documentation for what refuses.
    pub(crate) fn composite_export(&self, node: &Node<T>) -> Option<ExportedComposite> {
        if !node.may_have_animations() {
            return None;
        }
        let style = self.paint_style(node.id())?;
        let handle = self.animations().context_handle();
        let sets = handle.sets.read();
        let key = AnimationSetKey::new_for_non_pseudo(OpaqueNode(node.id().arena_key()));
        let set = sets.get(&key)?;
        // A transition restyles the element per frame regardless, and a
        // second running animation would need track merging.
        if !set.transitions.is_empty() {
            return None;
        }
        let mut running = None;
        for animation in &set.animations {
            match animation.state {
                AnimationState::Running => {
                    if running.replace(animation).is_some() {
                        return None;
                    }
                }
                AnimationState::Finished | AnimationState::Canceled => {}
                AnimationState::Pending | AnimationState::Paused(_) => return None,
            }
        }
        let animation = running?;

        // The element-level default timing function is per animation index.
        let ui = style.get_ui();
        let index = ui
            .animation_name_iter()
            .position(|name| name.as_atom() == Some(&animation.name))?;
        let default_easing = convert_computed_timing(&ui.animation_timing_function_mod(index))?;

        let keyframes = self
            .style_engine()
            .stylist()
            .lookup_keyframes(&animation.name, node)?;
        let guard = self.style_engine().shared_lock().read();

        // Collapse same-percentage steps, later declarations winning — the
        // collapse `IntermediateComputedKeyframe` applies.
        let mut points: Vec<StepPoint> = Vec::new();
        for step in &keyframes.steps {
            let converted = convert_step(step, &guard, default_easing)?;
            match points
                .iter_mut()
                .find(|existing| existing.percentage == converted.percentage)
            {
                Some(existing) => existing.merge(converted),
                None => points.push(converted),
            }
        }
        points.sort_by(|a, b| {
            a.percentage
                .partial_cmp(&b.percentage)
                .expect("keyframe percentages are finite")
        });

        let opacity = build_track(&points, |point| point.opacity)?;
        let transform_track = build_track(&points, |point| point.transform.clone())?;
        if opacity.is_none() && transform_track.is_none() {
            // Nothing this animation declares is a composite property.
            return None;
        }
        if let Some(track) = &transform_track
            && !lists_matched(track)
        {
            return None;
        }

        let committed_transform = if transform_track.is_some() {
            convert_computed_transform(style)?
        } else {
            Vec::new()
        };

        let curve = timed_curve(animation, opacity)?;
        Some(ExportedComposite {
            curve,
            transform_track,
            committed_transform,
        })
    }
}

/// The exported curve's timing shell around its tracks, from the public
/// `Animation` fields. `None` when the timing itself is inexportable: a
/// zero or unbounded duration, or no iterations left.
fn timed_curve(
    animation: &stylo::servo::animation::Animation,
    opacity: Option<Track<f32>>,
) -> Option<CompositeCurve> {
    let remaining = match animation.iteration_state {
        KeyframesIterationState::Finite(current, max) => {
            let remaining = (max - current).max(0.0);
            if remaining <= 0.0 {
                return None;
            }
            Iterations::Finite(remaining)
        }
        KeyframesIterationState::Infinite(_) => Iterations::Infinite,
    };
    if !(animation.duration > 0.0 && animation.duration.is_finite()) {
        return None;
    }
    let expires_at = match remaining {
        Iterations::Finite(count) => Some(animation.started_at + animation.duration * count),
        Iterations::Infinite => None,
    };
    Some(CompositeCurve {
        started_at: animation.started_at,
        duration: animation.duration,
        iterations: remaining,
        direction: DirectionState {
            reversed: animation.current_direction == AnimationDirection::Reverse,
            alternates: matches!(
                animation.direction,
                AnimationDirection::Alternate | AnimationDirection::AlternateReverse
            ),
        },
        expires_at,
        opacity,
        transform: None,
    })
}

/// One collapsed keyframe step in the exporter's vocabulary.
struct StepPoint {
    percentage: f64,
    easing: Easing,
    opacity: Option<f32>,
    transform: Option<TransformList>,
}

impl StepPoint {
    fn merge(&mut self, later: Self) {
        self.easing = later.easing;
        if later.opacity.is_some() {
            self.opacity = later.opacity;
        }
        if later.transform.is_some() {
            self.transform = later.transform;
        }
    }
}

/// A per-property track over the declaring points; `None` when the property
/// is never declared, refusal when it is declared but not at both ends.
#[expect(
    clippy::option_option,
    reason = "the outer level is the export refusal; the inner is whether \
              this property has a track at all"
)]
fn build_track<V: Clone>(
    points: &[StepPoint],
    value_of: impl Fn(&StepPoint) -> Option<V>,
) -> Option<Option<Track<V>>> {
    let declaring: Vec<TrackPoint<V>> = points
        .iter()
        .filter_map(|point| {
            value_of(point).map(|value| TrackPoint {
                percentage: point.percentage,
                value,
                easing: point.easing,
            })
        })
        .collect();
    if declaring.is_empty() {
        return Some(None);
    }
    let first = declaring.first().expect("non-empty").percentage;
    let last = declaring.last().expect("non-empty").percentage;
    if first != 0.0 || last != 1.0 {
        // Stylo would backfill from the start-time base style, which is not
        // recoverable here.
        return None;
    }
    Some(Some(Track { points: declaring }))
}

/// Every adjacent pair of a transform track must interpolate componentwise.
fn lists_matched(track: &Track<TransformList>) -> bool {
    track.points.windows(2).all(|pair| {
        pair[0].value.len() == pair[1].value.len()
            && pair[0]
                .value
                .iter()
                .zip(&pair[1].value)
                .all(|(a, b)| std::mem::discriminant(a) == std::mem::discriminant(b))
    })
}

fn convert_step(
    step: &KeyframesStep,
    guard: &SharedRwLockReadGuard<'_>,
    default_easing: Easing,
) -> Option<StepPoint> {
    let KeyframesStepValue::Declarations { block } = &step.value else {
        return None;
    };
    let easing = match step.get_animation_timing_function(guard) {
        Some(specified) => convert_specified_timing(&specified)?,
        None => default_easing,
    };
    let mut point = StepPoint {
        percentage: f64::from(step.start_offset.percentage.0),
        easing,
        opacity: None,
        transform: None,
    };
    for declaration in block.read_with(guard).declarations() {
        match declaration {
            PropertyDeclaration::Opacity(value) => {
                point.opacity = Some(parse_literal_opacity(value)?);
            }
            PropertyDeclaration::Transform(value) => {
                let ops: &[SpecifiedTransformOperation] = &value.0;
                let mut list = Vec::with_capacity(ops.len());
                for op in ops {
                    list.push(convert_specified_op(op)?);
                }
                point.transform = Some(list);
            }
            PropertyDeclaration::AnimationTimingFunction(_) => {}
            // Any other declaration means the animation moves a
            // non-composite property.
            _ => return None,
        }
    }
    Some(point)
}

type SpecifiedTransformOperation = stylo::values::specified::transform::TransformOperation;

fn convert_specified_op(op: &SpecifiedTransformOperation) -> Option<TransformOp> {
    use stylo::values::specified::LengthPercentage;
    let px = |value: &LengthPercentage| match value {
        LengthPercentage::Length(length) => length.to_px_if_absolute().map(f64::from),
        LengthPercentage::Percentage(_) | LengthPercentage::Calc(_) => None,
    };
    Some(match op {
        GenericTransformOperation::TranslateX(x) => TransformOp::TranslateX(px(x)?),
        GenericTransformOperation::TranslateY(y) => TransformOp::TranslateY(px(y)?),
        GenericTransformOperation::Translate(x, y) => TransformOp::Translate(px(x)?, px(y)?),
        GenericTransformOperation::ScaleX(x) => TransformOp::ScaleX(f64::from(x.get()?)),
        GenericTransformOperation::ScaleY(y) => TransformOp::ScaleY(f64::from(y.get()?)),
        GenericTransformOperation::Scale(x, y) => {
            TransformOp::Scale(f64::from(x.get()?), f64::from(y.get()?))
        }
        GenericTransformOperation::Rotate(angle) | GenericTransformOperation::RotateZ(angle) => {
            TransformOp::Rotate(f64::from(angle.degrees()?))
        }
        _ => return None,
    })
}

/// A specified `opacity` keyframe value, via its serialization: the field
/// is private upstream, and a literal value round-trips exactly. `calc()`
/// does not parse as a number and correctly refuses the export.
fn parse_literal_opacity(value: &stylo::values::specified::Opacity) -> Option<f32> {
    let css = value.to_css_string();
    if let Some(percent) = css.strip_suffix('%') {
        return percent.parse::<f32>().ok().map(|value| value / 100.0);
    }
    css.parse::<f32>().ok()
}

/// The committed style's transform, in the same vocabulary — so `Lc` is
/// built by the same code as `L(t)` and the delta closes exactly.
fn convert_computed_transform(style: &ComputedValues) -> Option<TransformList> {
    let ops: &[stylo::values::computed::transform::TransformOperation] =
        &style.get_box().transform.0;
    let mut list = Vec::with_capacity(ops.len());
    for op in ops {
        let px = |value: &stylo::values::computed::length_percentage::LengthPercentage| {
            value.to_length().map(|length| f64::from(length.px()))
        };
        list.push(match op {
            GenericTransformOperation::TranslateX(x) => TransformOp::TranslateX(px(x)?),
            GenericTransformOperation::TranslateY(y) => TransformOp::TranslateY(px(y)?),
            GenericTransformOperation::Translate(x, y) => TransformOp::Translate(px(x)?, px(y)?),
            GenericTransformOperation::ScaleX(x) => TransformOp::ScaleX(f64::from(*x)),
            GenericTransformOperation::ScaleY(y) => TransformOp::ScaleY(f64::from(*y)),
            GenericTransformOperation::Scale(x, y) => {
                TransformOp::Scale(f64::from(*x), f64::from(*y))
            }
            GenericTransformOperation::Rotate(angle)
            | GenericTransformOperation::RotateZ(angle) => {
                TransformOp::Rotate(f64::from(angle.degrees()))
            }
            _ => return None,
        });
    }
    Some(list)
}

fn convert_computed_timing(
    timing: &stylo::values::computed::easing::TimingFunction,
) -> Option<Easing> {
    match timing {
        TimingFunction::Keyword(keyword) => Some(keyword_easing(*keyword)),
        TimingFunction::CubicBezier { x1, y1, x2, y2 } => Some(Easing::CubicBezier {
            x1: *x1,
            y1: *y1,
            x2: *x2,
            y2: *y2,
        }),
        _ => None,
    }
}

fn convert_specified_timing(
    timing: &stylo::values::specified::easing::TimingFunction,
) -> Option<Easing> {
    match timing {
        TimingFunction::Keyword(keyword) => Some(keyword_easing(*keyword)),
        TimingFunction::CubicBezier { x1, y1, x2, y2 } => Some(Easing::CubicBezier {
            x1: x1.get()?,
            y1: y1.get()?,
            x2: x2.get()?,
            y2: y2.get()?,
        }),
        _ => None,
    }
}

/// The keyword control points from stylo's `calculate_output`.
fn keyword_easing(keyword: TimingKeyword) -> Easing {
    match keyword {
        TimingKeyword::Linear => Easing::Linear,
        TimingKeyword::Ease => Easing::CubicBezier {
            x1: 0.25,
            y1: 0.1,
            x2: 0.25,
            y2: 1.0,
        },
        TimingKeyword::EaseIn => Easing::CubicBezier {
            x1: 0.42,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
        },
        TimingKeyword::EaseOut => Easing::CubicBezier {
            x1: 0.0,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        },
        TimingKeyword::EaseInOut => Easing::CubicBezier {
            x1: 0.42,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        },
    }
}
