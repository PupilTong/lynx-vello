//! Exported composite-animation curves, sampled at compose time.
//!
//! A [`CompositeCurve`] is one element's active animation re-expressed in
//! plain data: per-iteration timing from the public `stylo` `Animation`
//! fields, and per-property keyframe tracks re-read from the stylist's
//! `@keyframes` steps. Sampling mirrors `stylo`'s
//! `Animation::get_property_declaration_at_time` — the same interval pick,
//! the same FROM-keyframe timing function, the same
//! `1 / (200 × segment duration)` bezier tolerance — so the value the
//! compositor draws between commits is the value the next commit's
//! main-thread restyle would have produced at the same instant. Anything the
//! exporter cannot re-express this way is simply not exported; the element
//! then animates through per-frame `BeginFrame` commits as before.
//!
//! The transform track's delta form: content was baked at the committed
//! transform `Lc`, so composition multiplies
//! `pre × L(t) × Lc⁻¹ × pre⁻¹` — with `pre` the element's world matrix up
//! to and including its transform origin — against everything the element
//! carries. At the commit instant `L(t) = Lc` and the delta is the
//! identity: composition reproduces the committed frame exactly.

#![expect(
    clippy::float_cmp,
    reason = "the exact-equality edges (progress at 0 and 1, identity \
              control points, zero spans) deliberately mirror stylo's \
              sampling code"
)]

use crate::vello::kurbo::Affine;

/// One `animation-timing-function` value the exporter can replay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Easing {
    Linear,
    CubicBezier { x1: f32, y1: f32, x2: f32, y2: f32 },
}

impl Easing {
    /// The eased output for `progress`, matching stylo's
    /// `TimingFunction::calculate_output` with `BeforeFlag::Unset`:
    /// `calculate_bezier_output`'s linear shortcut, exact edges, tangent
    /// extrapolation outside `[0, 1]`, and a Newton-then-bisection solve at
    /// the given tolerance inside it.
    fn output(self, progress: f64, epsilon: f64) -> f64 {
        match self {
            Self::Linear => progress,
            Self::CubicBezier { x1, y1, x2, y2 } => {
                bezier_output(progress, epsilon, x1, y1, x2, y2)
            }
        }
    }
}

/// One 2D transform operation in the exporter's vocabulary: the computed
/// variants whose interpolation is componentwise in stylo, with lengths
/// already in CSS px. Interval endpoints must match variant-for-variant —
/// exactly the lists stylo interpolates without a matrix fallback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TransformOp {
    TranslateX(f64),
    TranslateY(f64),
    Translate(f64, f64),
    ScaleX(f64),
    ScaleY(f64),
    Scale(f64, f64),
    /// Degrees, as stylo's computed `Angle` stores them.
    Rotate(f64),
}

impl TransformOp {
    fn interpolate(self, other: Self, progress: f64) -> Option<Self> {
        let lerp = |from: f64, to: f64| from + (to - from) * progress;
        Some(match (self, other) {
            (Self::TranslateX(a), Self::TranslateX(b)) => Self::TranslateX(lerp(a, b)),
            (Self::TranslateY(a), Self::TranslateY(b)) => Self::TranslateY(lerp(a, b)),
            (Self::Translate(ax, ay), Self::Translate(bx, by)) => {
                Self::Translate(lerp(ax, bx), lerp(ay, by))
            }
            (Self::ScaleX(a), Self::ScaleX(b)) => Self::ScaleX(lerp(a, b)),
            (Self::ScaleY(a), Self::ScaleY(b)) => Self::ScaleY(lerp(a, b)),
            (Self::Scale(ax, ay), Self::Scale(bx, by)) => Self::Scale(lerp(ax, bx), lerp(ay, by)),
            (Self::Rotate(a), Self::Rotate(b)) => Self::Rotate(lerp(a, b)),
            _ => return None,
        })
    }

    fn matrix(self) -> Affine {
        match self {
            Self::TranslateX(x) => Affine::translate((x, 0.0)),
            Self::TranslateY(y) => Affine::translate((0.0, y)),
            Self::Translate(x, y) => Affine::translate((x, y)),
            Self::ScaleX(x) => Affine::scale_non_uniform(x, 1.0),
            Self::ScaleY(y) => Affine::scale_non_uniform(1.0, y),
            Self::Scale(x, y) => Affine::scale_non_uniform(x, y),
            Self::Rotate(degrees) => Affine::rotate(degrees.to_radians()),
        }
    }
}

/// A transform list in the exporter's vocabulary, applied left to right.
pub(crate) type TransformList = Vec<TransformOp>;

pub(crate) fn transform_list_matrix(list: &[TransformOp]) -> Affine {
    let mut matrix = Affine::IDENTITY;
    for op in list {
        matrix *= op.matrix();
    }
    matrix
}

/// One declaring keyframe on one property's track.
#[derive(Debug, Clone)]
pub(crate) struct TrackPoint<V> {
    /// The keyframe's start percentage, `0..=1`.
    pub(crate) percentage: f64,
    pub(crate) value: V,
    /// This keyframe's timing function — applied when this point is the
    /// FROM of a sampled interval, exactly as stylo takes the walked-to
    /// declaring keyframe's function.
    pub(crate) easing: Easing,
}

/// One property's declaring keyframes, ascending by percentage, with both
/// ends present (the exporter refuses a track that does not declare `from`
/// and `to`).
#[derive(Debug, Clone)]
pub(crate) struct Track<V> {
    pub(crate) points: Vec<TrackPoint<V>>,
}

impl<V: Clone> Track<V> {
    /// The FROM point, TO point, and normalized progress for
    /// `total_progress` under `reversed`, mirroring stylo's interval pick
    /// plus per-property declaring-keyframe walk.
    fn interval(
        &self,
        total_progress: f64,
        reversed: bool,
    ) -> (&TrackPoint<V>, &TrackPoint<V>, f64) {
        let points = &self.points;
        let last = points.len() - 1;
        if reversed {
            // Reverse: progress runs over 1 − percentage.
            let next = points
                .iter()
                .rposition(|point| total_progress <= 1.0 - point.percentage)
                .unwrap_or(0);
            let prev = (next + 1).min(last);
            let span = (points[prev].percentage - points[next].percentage).abs();
            let progress = if span == 0.0 {
                0.0
            } else {
                (total_progress - (1.0 - points[prev].percentage)) / span
            };
            (&points[prev], &points[next], progress)
        } else {
            let next = points
                .iter()
                .position(|point| total_progress < point.percentage)
                .unwrap_or(last);
            let prev = next.saturating_sub(1);
            let span = (points[next].percentage - points[prev].percentage).abs();
            let progress = if span == 0.0 {
                0.0
            } else {
                (total_progress - points[prev].percentage) / span
            };
            (&points[prev], &points[next], progress)
        }
    }
}

/// How the animation iterates, from the public `Animation` fields.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Iterations {
    /// Iterations still to run as of the commit, including the current one's
    /// remainder.
    Finite(f64),
    Infinite,
}

/// Whether iteration parity flips the direction, and which way the current
/// iteration runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DirectionState {
    /// The commit-time iteration's direction: `true` is reverse.
    pub(crate) reversed: bool,
    /// `animation-direction: alternate | alternate-reverse` — parity flips.
    pub(crate) alternates: bool,
}

/// One element's exported animation: timing plus per-property tracks.
#[derive(Debug, Clone)]
pub(crate) struct CompositeCurve {
    /// When the commit-time iteration began, in timeline seconds — stylo's
    /// `Animation::started_at`, delay already inside.
    pub(crate) started_at: f64,
    /// One iteration, in seconds.
    pub(crate) duration: f64,
    pub(crate) iterations: Iterations,
    pub(crate) direction: DirectionState,
    /// The timeline second after which sampling leaves this curve's domain —
    /// the animation's end. `None` never ends locally (infinite iterations).
    pub(crate) expires_at: Option<f64>,
    pub(crate) opacity: Option<Track<f32>>,
    pub(crate) transform: Option<TransformTrack>,
}

/// The transform track plus the constant matrices its delta needs.
#[derive(Debug, Clone)]
pub(crate) struct TransformTrack {
    pub(crate) track: Track<TransformList>,
    /// The element's world matrix up to and including its transform origin.
    pub(crate) pre: Affine,
    pub(crate) pre_inverse: Affine,
    /// The inverse of the committed transform the frame was baked at.
    pub(crate) committed_inverse: Affine,
}

/// What one sample answers: the CSS-px delta against the committed bake and
/// the opacity replacing the committed one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CurveSample {
    pub(crate) delta: Affine,
    pub(crate) alpha: Option<f32>,
}

impl CompositeCurve {
    /// Samples the curve at timeline second `now`.
    pub(crate) fn sample(&self, now: f64) -> CurveSample {
        let (total_progress, reversed) = self.progress_at(now);
        let alpha = self.opacity.as_ref().map(|track| {
            self.sample_track(track, total_progress, reversed, |from, to, progress| {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "an interpolated opacity is within f32 range"
                )]
                let sampled =
                    (f64::from(*from) + (f64::from(*to) - f64::from(*from)) * progress) as f32;
                sampled
            })
        });
        let delta = self.transform.as_ref().map(|transform| {
            let list = self.sample_track(
                &transform.track,
                total_progress,
                reversed,
                |from, to, progress| {
                    let mut sampled = Vec::with_capacity(from.len());
                    for (a, b) in from.iter().zip(to) {
                        let Some(op) = a.interpolate(*b, progress) else {
                            // The exporter only admits matched lists; an
                            // unmatched pair here is a bug, but collapsing to
                            // the FROM op keeps the frame drawable.
                            debug_assert!(false, "exported transform lists must stay matched");
                            sampled.push(*a);
                            continue;
                        };
                        sampled.push(op);
                    }
                    sampled
                },
            );
            transform.pre
                * transform_list_matrix(&list)
                * transform.committed_inverse
                * transform.pre_inverse
        });
        CurveSample {
            delta: delta.unwrap_or(Affine::IDENTITY),
            alpha,
        }
    }

    /// Whether `now` is past this curve's domain — the compositor's cue to
    /// hand the animation back to the main thread for its finish restyle.
    pub(crate) fn expired_at(&self, now: f64) -> bool {
        self.expires_at.is_some_and(|expiry| now >= expiry)
    }

    /// The current-iteration progress at `now` plus the direction that
    /// iteration runs, emulating the `iterate_if_necessary` steps the main
    /// thread has not run: each whole elapsed iteration advances the start
    /// by one duration and flips parity when the direction alternates.
    fn progress_at(&self, now: f64) -> (f64, bool) {
        let raw = if self.duration > 0.0 {
            ((now - self.started_at) / self.duration).max(0.0)
        } else {
            f64::INFINITY
        };
        let remaining = match self.iterations {
            Iterations::Finite(remaining) => remaining,
            Iterations::Infinite => f64::INFINITY,
        };
        // Whole iterations elapsed since the commit, never past the last.
        let elapsed = raw.floor().min((remaining - 1.0).max(0.0).ceil());
        let elapsed = if elapsed.is_finite() { elapsed } else { 0.0 };
        let local = raw - elapsed;
        let end = (remaining - elapsed).clamp(0.0, 1.0);
        let total_progress = local.clamp(0.0, end.max(0.0));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "elapsed is a small non-negative whole number by construction"
        )]
        let flipped = self.direction.alternates && (elapsed as u64) % 2 == 1;
        (total_progress, self.direction.reversed ^ flipped)
    }

    fn sample_track<V: Clone, R>(
        &self,
        track: &Track<V>,
        total_progress: f64,
        reversed: bool,
        interpolate: impl Fn(&V, &V, f64) -> R,
    ) -> R {
        let points = &track.points;
        if total_progress >= 1.0 {
            let point = if reversed {
                &points[0]
            } else {
                &points[points.len() - 1]
            };
            return interpolate(&point.value, &point.value, 0.0);
        }
        let (from, to, progress) = track.interval(total_progress, reversed);
        let span = (to.percentage - from.percentage).abs();
        let segment_duration = span * self.duration;
        let epsilon = if segment_duration > 0.0 {
            1.0 / (200.0 * segment_duration)
        } else {
            1.0 / 200.0
        };
        let eased = from.easing.output(progress, epsilon);
        interpolate(&from.value, &to.value, eased)
    }
}

/// Stylo's `calculate_bezier_output`: linear shortcut when the curve is the
/// identity, exact values at the edges, tangent extrapolation outside
/// `[0, 1]`, otherwise a Newton solve with bisection fallback.
fn bezier_output(progress: f64, epsilon: f64, x1: f32, y1: f32, x2: f32, y2: f32) -> f64 {
    if x1 == y1 && x2 == y2 {
        return progress;
    }
    if progress == 0.0 {
        return 0.0;
    }
    if progress == 1.0 {
        return 1.0;
    }
    let (x1, y1, x2, y2) = (f64::from(x1), f64::from(y1), f64::from(x2), f64::from(y2));
    if progress < 0.0 {
        if x1 > 0.0 {
            return progress * y1 / x1;
        }
        if y1 == 0.0 && x2 > 0.0 {
            return progress * y2 / x2;
        }
        return 0.0;
    }
    if progress > 1.0 {
        if x2 < 1.0 {
            return 1.0 + (progress - 1.0) * (y2 - 1.0) / (x2 - 1.0);
        }
        if y2 == 1.0 && x1 < 1.0 {
            return 1.0 + (progress - 1.0) * (y1 - 1.0) / (x1 - 1.0);
        }
        return 1.0;
    }
    let bezier = Bezier::new(x1, y1, x2, y2);
    bezier.sample_y(bezier.solve_x(progress, epsilon))
}

/// The cubic solver from stylo's `bezier.rs`, polynomial coefficients and
/// all: Newton first, bisection fallback, the same `1e-6` derivative
/// bailout.
struct Bezier {
    ax: f64,
    bx: f64,
    cx: f64,
    ay: f64,
    by: f64,
    cy: f64,
}

impl Bezier {
    fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        let cx = 3.0 * x1;
        let bx = 3.0 * (x2 - x1) - cx;
        let cy = 3.0 * y1;
        let by = 3.0 * (y2 - y1) - cy;
        Self {
            ax: 1.0 - cx - bx,
            bx,
            cx,
            ay: 1.0 - cy - by,
            by,
            cy,
        }
    }

    fn sample_x(&self, t: f64) -> f64 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn sample_y(&self, t: f64) -> f64 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    fn sample_derivative_x(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    fn solve_x(&self, x: f64, epsilon: f64) -> f64 {
        let mut t = x;
        for _ in 0..8 {
            let x2 = self.sample_x(t) - x;
            if x2.abs() < epsilon {
                return t;
            }
            let derivative = self.sample_derivative_x(t);
            if derivative.abs() < 1e-6 {
                break;
            }
            t -= x2 / derivative;
        }

        let (mut low, mut high) = (0.0_f64, 1.0_f64);
        t = x;
        if t < low {
            return low;
        }
        if t > high {
            return high;
        }
        while low < high {
            let x2 = self.sample_x(t);
            if (x2 - x).abs() < epsilon {
                return t;
            }
            if x > x2 {
                low = t;
            } else {
                high = t;
            }
            t = (high - low) / 2.0 + low;
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opacity_curve(points: Vec<TrackPoint<f32>>) -> CompositeCurve {
        CompositeCurve {
            started_at: 0.0,
            duration: 1.0,
            iterations: Iterations::Finite(1.0),
            direction: DirectionState {
                reversed: false,
                alternates: false,
            },
            expires_at: Some(1.0),
            opacity: Some(Track { points }),
            transform: None,
        }
    }

    fn fade() -> CompositeCurve {
        opacity_curve(vec![
            TrackPoint {
                percentage: 0.0,
                value: 1.0,
                easing: Easing::Linear,
            },
            TrackPoint {
                percentage: 1.0,
                value: 0.0,
                easing: Easing::Linear,
            },
        ])
    }

    #[test]
    fn a_linear_opacity_track_interpolates_and_clamps() {
        let curve = fade();
        assert_eq!(curve.sample(0.0).alpha, Some(1.0));
        assert_eq!(curve.sample(0.25).alpha, Some(0.75));
        assert_eq!(curve.sample(2.0).alpha, Some(0.0));
        assert!(curve.expired_at(1.0));
        assert!(!curve.expired_at(0.5));
    }

    #[test]
    fn an_alternating_infinite_curve_flips_direction_per_iteration() {
        let mut curve = fade();
        curve.iterations = Iterations::Infinite;
        curve.direction.alternates = true;
        curve.expires_at = None;
        // Iteration 0 runs forward: opacity falls.
        assert_eq!(curve.sample(0.25).alpha, Some(0.75));
        // Iteration 1 runs reversed: the same local progress reads the track
        // from the other end, so opacity rises again.
        assert_eq!(curve.sample(1.25).alpha, Some(0.25));
        assert!(!curve.expired_at(1e6));
    }

    #[test]
    fn the_from_keyframe_timing_function_shapes_its_interval() {
        let curve = opacity_curve(vec![
            TrackPoint {
                percentage: 0.0,
                value: 0.0,
                // ease-in: slow start.
                easing: Easing::CubicBezier {
                    x1: 0.42,
                    y1: 0.0,
                    x2: 1.0,
                    y2: 1.0,
                },
            },
            TrackPoint {
                percentage: 1.0,
                value: 1.0,
                easing: Easing::Linear,
            },
        ]);
        let sampled = curve.sample(0.5).alpha.expect("an opacity track");
        assert!(
            sampled < 0.45,
            "ease-in at halfway stays under the linear value, got {sampled}"
        );
        let late = curve.sample(0.9).alpha.expect("an opacity track");
        assert!(late > 0.8, "ease-in catches up late, got {late}");
    }

    #[test]
    fn a_transform_track_composes_the_delta_against_the_committed_bake() {
        let track = Track {
            points: vec![
                TrackPoint {
                    percentage: 0.0,
                    value: vec![TransformOp::TranslateX(0.0)],
                    easing: Easing::Linear,
                },
                TrackPoint {
                    percentage: 1.0,
                    value: vec![TransformOp::TranslateX(100.0)],
                    easing: Easing::Linear,
                },
            ],
        };
        // Committed at t=0.25: baked at translateX(25).
        let committed = transform_list_matrix(&[TransformOp::TranslateX(25.0)]);
        let curve = CompositeCurve {
            started_at: 0.0,
            duration: 1.0,
            iterations: Iterations::Finite(1.0),
            direction: DirectionState {
                reversed: false,
                alternates: false,
            },
            expires_at: Some(1.0),
            opacity: None,
            transform: Some(TransformTrack {
                track,
                pre: Affine::IDENTITY,
                pre_inverse: Affine::IDENTITY,
                committed_inverse: committed.inverse(),
            }),
        };
        // At the commit instant the delta is the identity.
        let at_commit = curve.sample(0.25).delta;
        assert!((at_commit.as_coeffs()[4]).abs() < 1e-9, "{at_commit:?}");
        // Halfway: sampled translateX(50) against the baked 25 → +25px.
        let halfway = curve.sample(0.5).delta;
        assert!((halfway.as_coeffs()[4] - 25.0).abs() < 1e-9, "{halfway:?}");
    }

    #[test]
    fn a_rotation_delta_conjugates_through_the_transform_origin() {
        // An element whose origin sits at (50, 50): pre carries the origin
        // shift, so a quarter turn pivots there instead of the viewport
        // corner.
        let pre = Affine::translate((50.0, 50.0));
        let curve = CompositeCurve {
            started_at: 0.0,
            duration: 1.0,
            iterations: Iterations::Finite(1.0),
            direction: DirectionState {
                reversed: false,
                alternates: false,
            },
            expires_at: Some(1.0),
            opacity: None,
            transform: Some(TransformTrack {
                track: Track {
                    points: vec![
                        TrackPoint {
                            percentage: 0.0,
                            value: vec![TransformOp::Rotate(0.0)],
                            easing: Easing::Linear,
                        },
                        TrackPoint {
                            percentage: 1.0,
                            value: vec![TransformOp::Rotate(180.0)],
                            easing: Easing::Linear,
                        },
                    ],
                },
                pre,
                pre_inverse: pre.inverse(),
                committed_inverse: Affine::IDENTITY,
            }),
        };
        let half_turn = curve.sample(1.0).delta;
        let moved = half_turn * crate::vello::kurbo::Point::new(50.0, 0.0);
        assert!(
            (moved.x - 50.0).abs() < 1e-6 && (moved.y - 100.0).abs() < 1e-6,
            "a half turn about (50, 50) sends (50, 0) to (50, 100), got {moved:?}"
        );
    }
}
