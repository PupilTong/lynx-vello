//! The reach of an exported transform track: the range each op's parameters
//! take over the curve's whole domain, and a region pulled back through every
//! delta that range allows.
//!
//! Culling reads it: content below an animation node encodes when it can meet
//! the admitted region at some instant of the curve, so one encode serves the
//! whole domain. Each op's parameters range independently, which only widens
//! the set — the ranges are a superset of what one progress value produces.

use super::curves::{DirectionState, Easing, Track, TransformList, TransformOp};
use crate::vello::kurbo::{Affine, Point, Rect};

/// A closed interval `[low, high]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Interval {
    pub(crate) low: f64,
    pub(crate) high: f64,
}

impl Interval {
    const fn point(value: f64) -> Self {
        Self {
            low: value,
            high: value,
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            low: self.low.min(other.low),
            high: self.high.max(other.high),
        }
    }

    /// `from + (to − from)·y` over `y` in `self`.
    fn lerp(self, from: f64, to: f64) -> Self {
        let a = from + (to - from) * self.low;
        let b = from + (to - from) * self.high;
        Self {
            low: a.min(b),
            high: a.max(b),
        }
    }

    /// `1 − y` over `y` in `self`.
    fn flipped(self) -> Self {
        Self {
            low: 1.0 - self.high,
            high: 1.0 - self.low,
        }
    }

    fn contains_zero(self) -> bool {
        self.low <= 0.0 && self.high >= 0.0
    }
}

/// Every eased progress `easing` yields over progress `[0, 1]`: a cubic
/// Bézier stays inside its control points' hull, overshoot included.
fn eased_range(easing: Easing) -> Interval {
    match easing {
        Easing::Linear => Interval {
            low: 0.0,
            high: 1.0,
        },
        Easing::CubicBezier { y1, y2, .. } => Interval {
            low: 0.0_f64.min(f64::from(y1)).min(f64::from(y2)),
            high: 1.0_f64.max(f64::from(y1)).max(f64::from(y2)),
        },
    }
}

/// One op's parameter ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OpReach {
    Translate(Interval, Interval),
    Scale(Interval, Interval),
    /// Degrees.
    Rotate(Interval),
}

impl OpReach {
    /// The op's parameters between keyframe values `from` and `to` over eased
    /// progress `progress`; `None` for unmatched variants.
    fn between(from: TransformOp, to: TransformOp, progress: Interval) -> Option<Self> {
        let lerp = |a, b| progress.lerp(a, b);
        let still = Interval::point(0.0);
        let unit = Interval::point(1.0);
        Some(match (from, to) {
            (TransformOp::TranslateX(a), TransformOp::TranslateX(b)) => {
                Self::Translate(lerp(a, b), still)
            }
            (TransformOp::TranslateY(a), TransformOp::TranslateY(b)) => {
                Self::Translate(still, lerp(a, b))
            }
            (TransformOp::Translate(ax, ay), TransformOp::Translate(bx, by)) => {
                Self::Translate(lerp(ax, bx), lerp(ay, by))
            }
            (TransformOp::ScaleX(a), TransformOp::ScaleX(b)) => Self::Scale(lerp(a, b), unit),
            (TransformOp::ScaleY(a), TransformOp::ScaleY(b)) => Self::Scale(unit, lerp(a, b)),
            (TransformOp::Scale(ax, ay), TransformOp::Scale(bx, by)) => {
                Self::Scale(lerp(ax, bx), lerp(ay, by))
            }
            (TransformOp::Rotate(a), TransformOp::Rotate(b)) => Self::Rotate(lerp(a, b)),
            _ => return None,
        })
    }

    fn union(self, other: Self) -> Option<Self> {
        Some(match (self, other) {
            (Self::Translate(ax, ay), Self::Translate(bx, by)) => {
                Self::Translate(ax.union(bx), ay.union(by))
            }
            (Self::Scale(ax, ay), Self::Scale(bx, by)) => Self::Scale(ax.union(bx), ay.union(by)),
            (Self::Rotate(a), Self::Rotate(b)) => Self::Rotate(a.union(b)),
            _ => return None,
        })
    }

    /// The bounding box of every point this op maps into `region` at some
    /// parameter in range.
    fn pull_back(self, region: Rect) -> Rect {
        match self {
            Self::Translate(x, y) => Rect::new(
                region.x0 - x.high,
                region.y0 - y.high,
                region.x1 - x.low,
                region.y1 - y.low,
            ),
            Self::Scale(x, y) => {
                let (x0, x1) = unscale(region.x0, region.x1, x);
                let (y0, y1) = unscale(region.y0, region.y1, y);
                Rect::new(x0, y0, x1, y1)
            }
            Self::Rotate(degrees) => swept(
                region,
                Interval {
                    low: -degrees.high.to_radians(),
                    high: -degrees.low.to_radians(),
                },
            ),
        }
    }
}

/// `[low, high] / s` over `s` in `scale`, which excludes 0: `1/s` is
/// monotone there, so the extremes sit at the ends of both ranges.
fn unscale(low: f64, high: f64, scale: Interval) -> (f64, f64) {
    let ends = [
        low / scale.low,
        low / scale.high,
        high / scale.low,
        high / scale.high,
    ];
    (
        ends.into_iter().fold(f64::INFINITY, f64::min),
        ends.into_iter().fold(f64::NEG_INFINITY, f64::max),
    )
}

/// The bounding box of `region` rotated about the origin by every angle in
/// `angles` (radians): the union of its corners' arcs, since each rotated
/// box is bounded by its rotated corners.
fn swept(region: Rect, angles: Interval) -> Rect {
    let corners = [
        Point::new(region.x0, region.y0),
        Point::new(region.x1, region.y0),
        Point::new(region.x0, region.y1),
        Point::new(region.x1, region.y1),
    ];
    corners
        .into_iter()
        .map(|corner| arc_bounds(corner, angles))
        .reduce(|united, arc| united.union(arc))
        .expect("a rect has corners")
}

/// The bounding box of `point` rotated about the origin by every angle in
/// `angles`: the arc's two ends plus each axis direction it passes.
fn arc_bounds(point: Point, angles: Interval) -> Rect {
    let radius = point.to_vec2().hypot();
    let span = angles.high - angles.low;
    if span.is_nan() || span >= std::f64::consts::TAU {
        return Rect::new(-radius, -radius, radius, radius);
    }
    let start = Affine::rotate(angles.low) * point;
    let end = Affine::rotate(angles.high) * point;
    let mut bounds = Rect::from_points(start, end);
    // Headings run from `start`'s, in `[−π, π]`, through `span`: read off the
    // rotated point, not summed with `angles.low`, which may be too large to
    // add a heading to. Under a full turn, at most four axis directions lie
    // in between.
    let from = start.y.atan2(start.x);
    let quarter = std::f64::consts::FRAC_PI_2;
    let first = (from / quarter).ceil();
    for step in (0_u8..4).map(|offset| first + f64::from(offset)) {
        if step * quarter > from + span {
            break;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a quarter-turn count in [−2, 5]"
        )]
        let axis = match (step as i64).rem_euclid(4) {
            0 => Point::new(radius, 0.0),
            1 => Point::new(0.0, radius),
            2 => Point::new(-radius, 0.0),
            _ => Point::new(0.0, -radius),
        };
        bounds = bounds.union_pt(axis);
    }
    bounds
}

/// Where a transform track's delta `pre·L(t)·Lc⁻¹·pre⁻¹` can carry content,
/// over the curve's whole domain.
#[derive(Debug, Clone)]
pub(crate) struct Reach {
    /// `pre⁻¹`: the space above the node into the transform list's output.
    lower: Affine,
    /// Per op of `L`, in list order.
    ops: Vec<OpReach>,
    /// `pre·Lc`: the transform list's input into the node's space.
    lift: Affine,
}

impl Reach {
    /// The reach of `track`, run in `direction`, on an element whose world is
    /// `pre·L·origin⁻¹` committed at `L = committed`.
    ///
    /// A segment samples its FROM keyframe's easing: the lower one running
    /// forward, the upper one — over flipped progress — running reversed.
    /// Iterations and fill only pick progress inside `[0, 1]`.
    ///
    /// `None` when a scale range reaches or crosses 0: a vanishing scale
    /// draws content in from arbitrarily far.
    pub(crate) fn of(
        track: &Track<TransformList>,
        direction: DirectionState,
        pre: Affine,
        committed: Affine,
    ) -> Option<Self> {
        let first = track.points.first()?;
        let mut ops: Vec<OpReach> = first
            .value
            .iter()
            .map(|&op| OpReach::between(op, op, Interval::point(0.0)))
            .collect::<Option<_>>()?;
        for pair in track.points.windows(2) {
            let [from, to] = pair else {
                unreachable!("windows of two")
            };
            let forward = eased_range(from.easing);
            let reversed = eased_range(to.easing).flipped();
            let progress = if direction.alternates {
                forward.union(reversed)
            } else if direction.reversed {
                reversed
            } else {
                forward
            };
            if from.value.len() != ops.len() || to.value.len() != ops.len() {
                return None;
            }
            for ((op, &a), &b) in ops.iter_mut().zip(&from.value).zip(&to.value) {
                *op = op.union(OpReach::between(a, b, progress)?)?;
            }
        }
        let bounded = ops.iter().all(|op| match *op {
            OpReach::Scale(x, y) => !x.contains_zero() && !y.contains_zero(),
            OpReach::Translate(..) | OpReach::Rotate(_) => true,
        });
        bounded.then(|| Self {
            lower: pre.inverse(),
            ops,
            lift: pre * committed,
        })
    }

    /// The bounding box, in the node's space, of every point some delta in
    /// reach maps into `region`, given in the space above the node.
    pub(crate) fn pull_back(&self, region: Rect) -> Rect {
        let mut region = self.lower.transform_rect_bbox(region);
        // `L = op₁·op₂·…` applies op₁ last, so op₁ is undone first.
        for op in &self.ops {
            region = op.pull_back(region);
        }
        self.lift.transform_rect_bbox(region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual::curves::TrackPoint;

    const FORWARD: DirectionState = DirectionState {
        reversed: false,
        alternates: false,
    };

    fn track(points: &[(f64, TransformList, Easing)]) -> Track<TransformList> {
        Track {
            points: points
                .iter()
                .map(|(percentage, value, easing)| TrackPoint {
                    percentage: *percentage,
                    value: value.clone(),
                    easing: *easing,
                })
                .collect(),
        }
    }

    fn reach(points: &[(f64, TransformList, Easing)], direction: DirectionState) -> Option<Reach> {
        Reach::of(
            &track(points),
            direction,
            Affine::IDENTITY,
            Affine::IDENTITY,
        )
    }

    fn assert_rect(got: Rect, want: Rect) {
        let error = (got.x0 - want.x0)
            .abs()
            .max((got.y0 - want.y0).abs())
            .max((got.x1 - want.x1).abs())
            .max((got.y1 - want.y1).abs());
        assert!(error < 1e-9, "got {got:?}, want {want:?}");
    }

    /// `ease-in-back`-like control points dip below 0 and past 1: the hull
    /// covers the overshoot on both sides.
    #[test]
    fn a_bezier_segment_reaches_its_control_point_overshoot() {
        let overshoot = Easing::CubicBezier {
            x1: 0.5,
            y1: -0.5,
            x2: 0.5,
            y2: 1.5,
        };
        let reach = reach(
            &[
                (0.0, vec![TransformOp::TranslateX(0.0)], overshoot),
                (1.0, vec![TransformOp::TranslateX(100.0)], Easing::Linear),
            ],
            FORWARD,
        )
        .expect("a translation is bounded");
        assert_eq!(
            reach.ops,
            [OpReach::Translate(
                Interval {
                    low: -50.0,
                    high: 150.0
                },
                Interval::point(0.0),
            )],
        );
        // A region at x = 140 is reached by the overshoot alone.
        let region = Rect::new(240.0, 0.0, 250.0, 10.0);
        assert_rect(reach.pull_back(region), Rect::new(90.0, 0.0, 300.0, 10.0));
    }

    /// Running reversed, a segment eases by its upper keyframe's function
    /// over flipped progress.
    #[test]
    fn a_reversed_segment_eases_by_its_upper_keyframe() {
        let dip = Easing::CubicBezier {
            x1: 0.5,
            y1: -1.0,
            x2: 0.5,
            y2: 1.0,
        };
        let points = [
            (0.0, vec![TransformOp::TranslateX(0.0)], Easing::Linear),
            (1.0, vec![TransformOp::TranslateX(100.0)], dip),
        ];
        let forward = reach(&points, FORWARD).expect("bounded");
        assert_eq!(
            forward.ops[0],
            OpReach::Translate(
                Interval {
                    low: 0.0,
                    high: 100.0
                },
                Interval::point(0.0)
            ),
        );
        let reversed = reach(
            &points,
            DirectionState {
                reversed: true,
                alternates: false,
            },
        )
        .expect("bounded");
        // Reversed, `100 + (0 − 100)·y` over `y ∈ [−1, 1]` reaches 200.
        assert_eq!(
            reversed.ops[0],
            OpReach::Translate(
                Interval {
                    low: 0.0,
                    high: 200.0
                },
                Interval::point(0.0)
            ),
        );
    }

    /// `translate(100px, 0) rotate(90deg)` rotates first and translates
    /// last, so a pullback undoes the translation first.
    #[test]
    fn a_pullback_undoes_the_last_applied_op_first() {
        let list = vec![
            TransformOp::Translate(100.0, 0.0),
            TransformOp::Rotate(90.0),
        ];
        let reach = reach(
            &[
                (0.0, list.clone(), Easing::Linear),
                (1.0, list, Easing::Linear),
            ],
            FORWARD,
        )
        .expect("bounded");
        // L(50, 0) = translate(rotate(50, 0)) = (0, 50) + (100, 0).
        let region = Rect::new(99.0, 49.0, 101.0, 51.0);
        assert_rect(reach.pull_back(region), Rect::new(49.0, -1.0, 51.0, 1.0));
    }

    /// A sweep over 30°..120° carries a point through the +y axis, whose
    /// extreme neither end of the arc reaches.
    #[test]
    fn a_rotation_crossing_an_axis_reaches_the_axis_extreme() {
        let reach = reach(
            &[
                (0.0, vec![TransformOp::Rotate(-120.0)], Easing::Linear),
                (1.0, vec![TransformOp::Rotate(-30.0)], Easing::Linear),
            ],
            FORWARD,
        )
        .expect("bounded");
        // Undoing −120°..−30° rotates by 30°..120°: (10, 0) passes (0, 10).
        let pulled = reach.pull_back(Rect::new(10.0, 0.0, 10.0, 0.0));
        let (sin30, cos30) = 30_f64.to_radians().sin_cos();
        assert_rect(
            pulled,
            Rect::new(-10.0 * sin30, 10.0 * sin30, 10.0 * cos30, 10.0),
        );
    }

    /// A full turn or more is the bounding square of each corner's circle.
    #[test]
    fn a_full_turn_sweeps_the_whole_circle() {
        let reach = reach(
            &[
                (0.0, vec![TransformOp::Rotate(0.0)], Easing::Linear),
                (1.0, vec![TransformOp::Rotate(360.0)], Easing::Linear),
            ],
            FORWARD,
        )
        .expect("bounded");
        assert_rect(
            reach.pull_back(Rect::new(3.0, 4.0, 3.0, 4.0)),
            Rect::new(-5.0, -5.0, 5.0, 5.0),
        );
    }

    #[test]
    fn a_scale_range_divides_the_region_at_both_ends() {
        let reach = reach(
            &[
                (0.0, vec![TransformOp::Scale(0.5, 1.0)], Easing::Linear),
                (1.0, vec![TransformOp::Scale(2.0, 1.0)], Easing::Linear),
            ],
            FORWARD,
        )
        .expect("bounded");
        assert_rect(
            reach.pull_back(Rect::new(-10.0, -10.0, 40.0, 10.0)),
            Rect::new(-20.0, -10.0, 80.0, 10.0),
        );
    }

    #[test]
    fn a_scale_range_reaching_zero_is_unbounded() {
        for (from, to) in [(0.0, 1.0), (-1.0, 1.0)] {
            assert!(
                reach(
                    &[
                        (0.0, vec![TransformOp::ScaleY(from)], Easing::Linear),
                        (1.0, vec![TransformOp::ScaleY(to)], Easing::Linear),
                    ],
                    FORWARD,
                )
                .is_none(),
                "scaleY({from}) to scaleY({to})",
            );
        }
        // An overshooting ease carries `0.1 → 1` below 0 as well.
        let overshoot = Easing::CubicBezier {
            x1: 0.3,
            y1: -0.5,
            x2: 0.7,
            y2: 1.0,
        };
        assert!(
            reach(
                &[
                    (0.0, vec![TransformOp::Scale(0.1, 0.1)], overshoot),
                    (1.0, vec![TransformOp::Scale(1.0, 1.0)], Easing::Linear),
                ],
                FORWARD,
            )
            .is_none()
        );
    }

    /// `pre` and `Lc` conjugate the op ranges: a quarter-turn curve about an
    /// origin at (50, 50), committed at 0°, reaches a region about that
    /// origin only.
    #[test]
    fn the_fixed_maps_conjugate_the_op_ranges() {
        let pre = Affine::translate((50.0, 50.0));
        let reach = Reach::of(
            &track(&[
                (0.0, vec![TransformOp::Rotate(0.0)], Easing::Linear),
                (1.0, vec![TransformOp::Rotate(90.0)], Easing::Linear),
            ]),
            FORWARD,
            pre,
            Affine::IDENTITY,
        )
        .expect("bounded");
        // Content at (50 + 10·cos a, 50 + 10·sin a) for a in 0..90° is
        // carried onto (50, 60) at some instant.
        assert_rect(
            reach.pull_back(Rect::new(50.0, 60.0, 50.0, 60.0)),
            Rect::new(50.0, 50.0, 60.0, 60.0),
        );
    }

    /// Content baked mid-curve at `Lc = scale(1.5)` sits at `L(t)·Lc⁻¹` of
    /// its baked place, so the pullback lifts through `Lc`: over `s ∈ [1, 2]`,
    /// `x·s/1.5 ∈ [10, 30]` holds for `x ∈ [7.5, 45]`.
    #[test]
    fn a_curve_committed_mid_scale_lifts_through_the_committed_scale() {
        let reach = Reach::of(
            &track(&[
                (0.0, vec![TransformOp::Scale(1.0, 1.0)], Easing::Linear),
                (1.0, vec![TransformOp::Scale(2.0, 2.0)], Easing::Linear),
            ]),
            FORWARD,
            Affine::IDENTITY,
            Affine::scale(1.5),
        )
        .expect("bounded");
        assert_rect(
            reach.pull_back(Rect::new(10.0, 10.0, 30.0, 30.0)),
            Rect::new(7.5, 7.5, 45.0, 45.0),
        );
    }

    /// An angle too large to add a heading to still sweeps in bounded time:
    /// a constant rotation reaches its own rotation of the region alone.
    #[test]
    fn a_huge_constant_rotation_reaches_its_own_rotation() {
        let region = Rect::new(3.0, -4.0, 6.0, 8.0);
        for degrees in [1e18, 1e20, -1e20, 3.6e22] {
            let list = vec![TransformOp::Rotate(degrees)];
            let reach = reach(
                &[
                    (0.0, list.clone(), Easing::Linear),
                    (1.0, list, Easing::Linear),
                ],
                FORWARD,
            )
            .expect("bounded");
            assert_rect(
                reach.pull_back(region),
                Affine::rotate(-degrees.to_radians()).transform_rect_bbox(region),
            );
        }
    }

    /// Whole turns before the sweep change nothing: 30°..120° after ten
    /// thousand turns still passes the +y axis.
    #[test]
    fn a_sweep_after_many_turns_reaches_the_axis_it_passes() {
        let turns = 10_000.0 * 360.0;
        let reach = reach(
            &[
                (
                    0.0,
                    vec![TransformOp::Rotate(-turns - 120.0)],
                    Easing::Linear,
                ),
                (
                    1.0,
                    vec![TransformOp::Rotate(-turns - 30.0)],
                    Easing::Linear,
                ),
            ],
            FORWARD,
        )
        .expect("bounded");
        let pulled = reach.pull_back(Rect::new(10.0, 0.0, 10.0, 0.0));
        let (sin30, cos30) = 30_f64.to_radians().sin_cos();
        assert_rect(
            pulled,
            Rect::new(-10.0 * sin30, 10.0 * sin30, 10.0 * cos30, 10.0),
        );
    }
}
