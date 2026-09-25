//! The reach of an exported transform track: the range each op's parameters
//! take over the curve's whole domain, a region pulled back through every
//! delta that range allows, and content bounds carried forward through them.
//!
//! Culling reads the pullback: content below an animation node encodes when
//! it can meet the admitted region at some instant of the curve, so one
//! encode serves the whole domain. Group bounds read the forward carry: a
//! group whose content moves inside it must hold every place that content
//! can go. Each op's parameters range independently, which only widens the
//! set — the ranges are a superset of what one progress value produces.
//!
//! The ranges exist where stylo interpolates op by op: every contributor's
//! lists hold the same translate, scale and planar rotate primitives in the
//! same order, a shorter one padded with identities as stylo pads it. A list
//! holding an op the reach does not model (matrix, skew, 3D rotation) or a
//! mismatched remainder stylo decomposes has no reach: its pullback admits
//! everything, and the per-element extent budget is what bounds its encode.

use euclid::default::Size2D;
use stylo::values::computed::transform::{
    Transform as ComputedTransform, TransformOperation as ComputedTransformOperation,
};
use stylo::values::computed::{CSSPixelLength, TimingFunction};
use stylo::values::generics::transform::GenericTransformOperation;

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

    pub(crate) fn union(self, other: Self) -> Self {
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
    pub(crate) fn flipped(self) -> Self {
        Self {
            low: 1.0 - self.high,
            high: 1.0 - self.low,
        }
    }

    fn contains_zero(self) -> bool {
        self.low <= 0.0 && self.high >= 0.0
    }

    /// The smallest `|y|` over `y` in `self`, which excludes 0.
    fn least_magnitude(self) -> f64 {
        self.low.abs().min(self.high.abs())
    }

    /// The largest `|y|` over `y` in `self`.
    fn greatest_magnitude(self) -> f64 {
        self.low.abs().max(self.high.abs())
    }
}

/// Every eased progress `timing` yields over progress `[0, 1]`: a cubic
/// Bézier stays inside its control points' hull, overshoot included, and
/// steps inside `[0, 1]`. `None` for `linear()`, which the Lynx grammar does
/// not parse.
pub(crate) fn eased_range(timing: &TimingFunction) -> Option<Interval> {
    let hull = |y1: f32, y2: f32| Interval {
        low: 0.0_f64.min(f64::from(y1)).min(f64::from(y2)),
        high: 1.0_f64.max(f64::from(y1)).max(f64::from(y2)),
    };
    Some(match *timing {
        // Every keyword's control points lie inside the unit square.
        TimingFunction::Keyword(_) | TimingFunction::Steps(..) => Interval {
            low: 0.0,
            high: 1.0,
        },
        TimingFunction::CubicBezier { y1, y2, .. } => hull(y1, y2),
        // The cubic stylo evaluates it as.
        TimingFunction::SquareBezier { y, .. } => {
            hull(y * (2.0 / 3.0), 1.0 + (y - 1.0) * (2.0 / 3.0))
        }
        TimingFunction::LinearFunction(_) => return None,
    })
}

/// One segment a transform contributor interpolates over, with the eased
/// progress it samples there; `None` when that is unknown.
#[derive(Clone, Copy)]
pub(crate) struct Segment<'a> {
    pub(crate) from: &'a ComputedTransform,
    pub(crate) to: &'a ComputedTransform,
    pub(crate) eased: Option<Interval>,
}

/// The transform primitives with a reach, by stylo's matching: any two of
/// one kind interpolate op by op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Translate,
    Scale,
    /// About z, where a 2D composition sees it.
    Rotate,
}

impl Kind {
    /// The parameters of this kind's identity, which pads a shorter list.
    const fn identity(self) -> [f64; 2] {
        match self {
            Self::Translate | Self::Rotate => [0.0, 0.0],
            Self::Scale => [1.0, 1.0],
        }
    }
}

/// One op's planar part: translation in CSS px, scale factors, or a
/// rotation's degrees in the first slot.
#[derive(Debug, Clone, Copy)]
struct Op {
    kind: Kind,
    params: [f64; 2],
}

impl Op {
    /// `op`'s planar part, `%` resolved against `reference`; `None` for an op
    /// without a reach.
    fn of(op: &ComputedTransformOperation, reference: Size2D<f32>) -> Option<Self> {
        use GenericTransformOperation as T;
        let width = |length: &stylo::values::computed::LengthPercentage| {
            f64::from(length.resolve(CSSPixelLength::new(reference.width)).px())
        };
        let height = |length: &stylo::values::computed::LengthPercentage| {
            f64::from(length.resolve(CSSPixelLength::new(reference.height)).px())
        };
        let (kind, params) = match op {
            T::TranslateX(x) => (Kind::Translate, [width(x), 0.0]),
            T::TranslateY(y) => (Kind::Translate, [0.0, height(y)]),
            T::Translate(x, y) | T::Translate3D(x, y, _) => {
                (Kind::Translate, [width(x), height(y)])
            }
            T::TranslateZ(_) => (Kind::Translate, [0.0, 0.0]),
            T::ScaleX(x) => (Kind::Scale, [f64::from(*x), 1.0]),
            T::ScaleY(y) => (Kind::Scale, [1.0, f64::from(*y)]),
            T::Scale(x, y) | T::Scale3D(x, y, _) => (Kind::Scale, [f64::from(*x), f64::from(*y)]),
            T::ScaleZ(_) => (Kind::Scale, [1.0, 1.0]),
            T::Rotate(angle) | T::RotateZ(angle) => {
                (Kind::Rotate, [f64::from(angle.degrees()), 0.0])
            }
            _ => return None,
        };
        Some(Self { kind, params })
    }

    const fn identity(kind: Kind) -> Self {
        Self {
            kind,
            params: kind.identity(),
        }
    }
}

/// One op's parameter ranges: the second unused for a rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OpReach {
    kind: Kind,
    ranges: [Interval; 2],
}

impl OpReach {
    /// The parameters between `from` and `to`, of one kind, over eased
    /// progress `eased`.
    fn between(from: Op, to: Op, eased: Interval) -> Self {
        debug_assert_eq!(from.kind, to.kind, "the shape pairs kinds");
        Self {
            kind: from.kind,
            ranges: [
                eased.lerp(from.params[0], to.params[0]),
                eased.lerp(from.params[1], to.params[1]),
            ],
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            kind: self.kind,
            ranges: [
                self.ranges[0].union(other.ranges[0]),
                self.ranges[1].union(other.ranges[1]),
            ],
        }
    }

    /// The bounding box of every point this op maps some point of `bounds`
    /// to at some parameter in range.
    fn carry(self, bounds: Rect) -> Rect {
        let [x, y] = self.ranges;
        match self.kind {
            Kind::Translate => Rect::new(
                bounds.x0 + x.low,
                bounds.y0 + y.low,
                bounds.x1 + x.high,
                bounds.y1 + y.high,
            ),
            Kind::Scale => {
                let (x0, x1) = rescale(bounds.x0, bounds.x1, x);
                let (y0, y1) = rescale(bounds.y0, bounds.y1, y);
                Rect::new(x0, y0, x1, y1)
            }
            Kind::Rotate => swept(
                bounds,
                Interval {
                    low: x.low.to_radians(),
                    high: x.high.to_radians(),
                },
            ),
        }
    }

    /// The bounding box of every point this op maps into `region` at some
    /// parameter in range.
    fn pull_back(self, region: Rect) -> Rect {
        let [x, y] = self.ranges;
        match self.kind {
            Kind::Translate => Rect::new(
                region.x0 - x.high,
                region.y0 - y.high,
                region.x1 - x.low,
                region.y1 - y.low,
            ),
            Kind::Scale => {
                let (x0, x1) = unscale(region.x0, region.x1, x);
                let (y0, y1) = unscale(region.y0, region.y1, y);
                Rect::new(x0, y0, x1, y1)
            }
            Kind::Rotate => swept(
                region,
                Interval {
                    low: -x.high.to_radians(),
                    high: -x.low.to_radians(),
                },
            ),
        }
    }
}

/// `[low, high] · s` over `s` in `scale`: the product is bilinear, so the
/// extremes sit at the ends of both ranges.
fn rescale(low: f64, high: f64, scale: Interval) -> (f64, f64) {
    let ends = [
        low * scale.low,
        low * scale.high,
        high * scale.low,
        high * scale.high,
    ];
    (
        ends.into_iter().fold(f64::INFINITY, f64::min),
        ends.into_iter().fold(f64::NEG_INFINITY, f64::max),
    )
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
/// over the curve's whole domain; see the module documentation for when it
/// has no bound.
#[derive(Debug, Clone)]
pub(crate) struct Reach(Option<Bounded>);

#[derive(Debug, Clone)]
struct Bounded {
    /// `pre`: the transform list's output into the space above the node.
    pre: Affine,
    pre_inverse: Affine,
    /// Per op of `L`, in list order.
    ops: Vec<OpReach>,
    /// `pre·Lc`: the transform list's input into the node's space.
    lift: Affine,
    lift_inverse: Affine,
    /// An upper bound on the spectral norm of every delta's inverse — how
    /// far one CSS px above the node can reach in its space. `None` when a
    /// scale range reaches or crosses 0.
    inverse_norm: Option<f64>,
    /// An upper bound on the spectral norm of every delta — how far one CSS
    /// px in the node's space can reach above it.
    norm: f64,
}

impl Reach {
    /// The reach of a track sampling `segments`, the committed list
    /// `committed` among them, on an element of border box `reference` whose
    /// world is `pre·L·origin⁻¹`, committed at `L = committed_matrix`.
    pub(crate) fn of<'a>(
        segments: impl IntoIterator<Item = Segment<'a>>,
        committed: &'a ComputedTransform,
        reference: Size2D<f32>,
        pre: Affine,
        committed_matrix: Affine,
    ) -> Self {
        let still = Segment {
            from: committed,
            to: committed,
            eased: Some(Interval::point(0.0)),
        };
        let ops = op_ranges(still, segments, reference);
        Self(ops.map(|ops| Bounded::new(ops, pre, committed_matrix)))
    }

    /// The reach of a curve that never moves.
    #[cfg(test)]
    pub(crate) fn still() -> Self {
        Self(Some(Bounded::new(
            Vec::new(),
            Affine::IDENTITY,
            Affine::IDENTITY,
        )))
    }

    /// Whether the reach bounds anything.
    pub(crate) fn is_bounded(&self) -> bool {
        self.0.is_some()
    }

    /// The bounding box, in the node's space, of every point some delta in
    /// reach maps into `region`, given in the space above the node. `None`
    /// when there is no bound, or a scale range reaches 0: a vanishing scale
    /// draws content in from arbitrarily far.
    pub(crate) fn pull_back(&self, region: Rect) -> Option<Rect> {
        let reach = self.0.as_ref()?;
        reach.inverse_norm?;
        let mut region = reach.pre_inverse.transform_rect_bbox(region);
        // `L = op₁·op₂·…` applies op₁ last, so op₁ is undone first.
        for op in &reach.ops {
            region = op.pull_back(region);
        }
        Some(reach.lift.transform_rect_bbox(region))
    }

    /// The bounding box, in the space above the node, of every point some
    /// delta in reach maps `bounds`, given in the node's space, to. Bounded
    /// whatever the scale range, 0 included; the whole plane without a
    /// reach, which the builder keeps out of every group.
    pub(crate) fn carry(&self, bounds: Rect) -> Rect {
        let Some(reach) = &self.0 else {
            return Rect::new(
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            );
        };
        let mut bounds = reach.lift_inverse.transform_rect_bbox(bounds);
        // The last op applies first.
        for op in reach.ops.iter().rev() {
            bounds = op.carry(bounds);
        }
        reach.pre.transform_rect_bbox(bounds)
    }

    /// An upper bound on how far one CSS px above the node can reach in its
    /// space under any delta in reach; `None` without a bound or when a
    /// scale range reaches 0.
    pub(crate) fn inverse_norm(&self) -> Option<f64> {
        self.0.as_ref()?.inverse_norm
    }

    /// An upper bound on how far one CSS px in the node's space can reach
    /// above it under any delta in reach. Finite whatever the scale range;
    /// infinite without a reach, which the builder keeps out of every group.
    pub(crate) fn norm(&self) -> f64 {
        self.0.as_ref().map_or(f64::INFINITY, |reach| reach.norm)
    }
}

impl Bounded {
    fn new(ops: Vec<OpReach>, pre: Affine, committed: Affine) -> Self {
        // `Δ⁻¹ = pre·Lc·L(t)⁻¹·pre⁻¹`, and the norm is submultiplicative:
        // translations and rotations leave distances alone, a scale divides
        // them by its least magnitude.
        let pre_inverse = pre.inverse();
        let conjugate = pre.spectral_norm() * pre_inverse.spectral_norm();
        let inverse_norm =
            ops.iter()
                .try_fold(conjugate * committed.spectral_norm(), |norm, op| {
                    match op.kind {
                        Kind::Scale if op.ranges.iter().any(|range| range.contains_zero()) => None,
                        Kind::Scale => Some(
                            norm / op.ranges[0]
                                .least_magnitude()
                                .min(op.ranges[1].least_magnitude()),
                        ),
                        Kind::Translate | Kind::Rotate => Some(norm),
                    }
                });
        // `Δ = pre·L(t)·Lc⁻¹·pre⁻¹`: a scale multiplies distances by its
        // greatest magnitude.
        let lift = pre * committed;
        let norm = ops.iter().fold(
            conjugate * committed.inverse().spectral_norm(),
            |norm, op| match op.kind {
                Kind::Scale => {
                    norm * op.ranges[0]
                        .greatest_magnitude()
                        .max(op.ranges[1].greatest_magnitude())
                }
                Kind::Translate | Kind::Rotate => norm,
            },
        );
        Self {
            pre,
            pre_inverse,
            ops,
            lift,
            lift_inverse: lift.inverse(),
            inverse_norm,
            norm,
        }
    }
}

/// One segment's ops paired as stylo pairs them: position by position while
/// the kinds match, the longer list's remainder against identities. `None`
/// for an op without a reach, a remainder on both sides — which stylo
/// decomposes as matrices — or an unknown eased range.
fn paired(segment: &Segment<'_>, reference: Size2D<f32>) -> Option<(Vec<(Op, Op)>, Interval)> {
    let eased = segment.eased?;
    let (from, to) = (&segment.from.0, &segment.to.0);
    let mut pairs = Vec::with_capacity(from.len().max(to.len()));
    for index in 0..from.len().max(to.len()) {
        // Absent is `Some(None)`; an op without a reach, `None`.
        let at = |list: &[ComputedTransformOperation]| match list.get(index) {
            Some(op) => Op::of(op, reference).map(Some),
            None => Some(None),
        };
        pairs.push(match (at(from)?, at(to)?) {
            (Some(from), Some(to)) if from.kind == to.kind => (from, to),
            (Some(from), None) => (from, Op::identity(from.kind)),
            (None, Some(to)) => (Op::identity(to.kind), to),
            _ => return None,
        });
    }
    Some((pairs, eased))
}

/// The per-op ranges over `committed` and every other segment, paired, when
/// their kinds agree position by position; a segment shorter than another
/// holds identities at the positions it lacks.
fn op_ranges<'a>(
    committed: Segment<'a>,
    segments: impl IntoIterator<Item = Segment<'a>>,
    reference: Size2D<f32>,
) -> Option<Vec<OpReach>> {
    let (pairs, eased) = paired(&committed, reference)?;
    let mut ops: Vec<OpReach> = pairs
        .into_iter()
        .map(|(from, to)| OpReach::between(from, to, eased))
        .collect();
    for segment in segments {
        let (pairs, eased) = paired(&segment, reference)?;
        for (index, op) in ops.iter_mut().enumerate() {
            let identity = Op::identity(op.kind);
            let (from, to) = pairs.get(index).copied().unwrap_or((identity, identity));
            if from.kind != op.kind {
                return None;
            }
            *op = op.union(OpReach::between(from, to, eased));
        }
        let known = ops.len();
        for &(from, to) in pairs.iter().skip(known) {
            // Every segment before this one holds the identity here.
            let identity = Op::identity(from.kind);
            let before = OpReach::between(identity, identity, Interval::point(0.0));
            ops.push(before.union(OpReach::between(from, to, eased)));
        }
    }
    Some(ops)
}

#[cfg(test)]
mod tests {
    use stylo::values::computed::{Angle, LengthPercentage};
    use stylo::values::generics::easing::StepPosition;

    use super::*;
    use crate::test_common::Doc;

    type Operation = ComputedTransformOperation;

    fn translate(x: f32, y: f32) -> Operation {
        GenericTransformOperation::Translate(
            LengthPercentage::new_length(CSSPixelLength::new(x)),
            LengthPercentage::new_length(CSSPixelLength::new(y)),
        )
    }

    fn translate_x(x: f32) -> Operation {
        GenericTransformOperation::TranslateX(LengthPercentage::new_length(CSSPixelLength::new(x)))
    }

    fn rotate(degrees: f32) -> Operation {
        GenericTransformOperation::Rotate(Angle::from_degrees(degrees))
    }

    fn scale(x: f32, y: f32) -> Operation {
        GenericTransformOperation::Scale(x, y)
    }

    fn list(ops: Vec<Operation>) -> ComputedTransform {
        stylo::values::generics::transform::Transform(ops.into())
    }

    const LINEAR: Interval = Interval {
        low: 0.0,
        high: 1.0,
    };

    /// The reach of `segments`, each `(from, to, eased)`, conjugated by
    /// `pre` and lifted by `committed`, committed at the first one's `from`.
    fn reach_of(
        segments: &[(ComputedTransform, ComputedTransform, Interval)],
        pre: Affine,
        committed: Affine,
    ) -> Reach {
        Reach::of(
            segments.iter().map(|(from, to, eased)| Segment {
                from,
                to,
                eased: Some(*eased),
            }),
            &segments[0].0,
            Size2D::new(100.0, 50.0),
            pre,
            committed,
        )
    }

    fn reach(segments: &[(ComputedTransform, ComputedTransform, Interval)]) -> Reach {
        reach_of(segments, Affine::IDENTITY, Affine::IDENTITY)
    }

    fn ops(reach: &Reach) -> &[OpReach] {
        &reach.0.as_ref().expect("a bounded reach").ops
    }

    fn translation(x: Interval, y: Interval) -> OpReach {
        OpReach {
            kind: Kind::Translate,
            ranges: [x, y],
        }
    }

    fn assert_rect(got: Rect, want: Rect) {
        let error = (got.x0 - want.x0)
            .abs()
            .max((got.y0 - want.y0).abs())
            .max((got.x1 - want.x1).abs())
            .max((got.y1 - want.y1).abs());
        assert!(error < 1e-9, "got {got:?}, want {want:?}");
    }

    /// A cubic Bézier's hull covers its overshoot; steps and every keyword
    /// stay in `[0, 1]`; `square-bezier` reads the cubic stylo converts it
    /// to.
    #[test]
    fn an_easing_reaches_its_control_point_hull() {
        let overshoot = TimingFunction::CubicBezier {
            x1: 0.5,
            y1: -0.5,
            x2: 0.5,
            y2: 1.5,
        };
        assert_eq!(
            eased_range(&overshoot),
            Some(Interval {
                low: -0.5,
                high: 1.5
            })
        );
        for unit in [
            TimingFunction::Steps(4, StepPosition::JumpBoth),
            TimingFunction::Keyword(stylo::values::generics::easing::TimingKeyword::Ease),
        ] {
            assert_eq!(eased_range(&unit), Some(LINEAR));
        }
        let square = eased_range(&TimingFunction::SquareBezier { x: 0.5, y: 2.5 })
            .expect("a cubic in disguise");
        assert!(
            (square.high - 2.0).abs() < 1e-6 && square.low == 0.0,
            "{square:?}"
        );
    }

    /// An overshooting segment reaches past both keyframes.
    #[test]
    fn a_bezier_segment_reaches_its_control_point_overshoot() {
        let reach = reach(&[(
            list(vec![translate_x(0.0)]),
            list(vec![translate_x(100.0)]),
            Interval {
                low: -0.5,
                high: 1.5,
            },
        )]);
        assert_eq!(
            ops(&reach),
            [translation(
                Interval {
                    low: -50.0,
                    high: 150.0
                },
                Interval::point(0.0),
            )],
        );
        // A region at x = 140 is reached by the overshoot alone.
        let region = Rect::new(240.0, 0.0, 250.0, 10.0);
        assert_rect(
            reach.pull_back(region).expect("bounded"),
            Rect::new(90.0, 0.0, 300.0, 10.0),
        );
    }

    /// `translateX` and `translate` are one primitive, `%` resolves against
    /// the border box, and a shorter list pads with identities, as stylo
    /// pairs them — `none` included.
    #[test]
    fn primitives_pair_and_pad_as_stylo_interpolates_them() {
        let half = GenericTransformOperation::TranslateY(LengthPercentage::new_percent(
            stylo::values::computed::Percentage(0.5),
        ));
        let reach = reach(&[
            (
                list(vec![translate_x(10.0)]),
                list(vec![translate(-20.0, 0.0), rotate(90.0)]),
                LINEAR,
            ),
            (list(vec![]), list(vec![half]), LINEAR),
        ]);
        assert_eq!(
            ops(&reach),
            [
                translation(
                    Interval {
                        low: -20.0,
                        high: 10.0
                    },
                    Interval {
                        low: 0.0,
                        high: 25.0
                    },
                ),
                OpReach {
                    kind: Kind::Rotate,
                    ranges: [
                        Interval {
                            low: 0.0,
                            high: 90.0
                        },
                        Interval::point(0.0)
                    ],
                },
            ],
        );
    }

    /// A list holding an op the reach does not model — a `matrix()`, a skew
    /// or a turn out of the plane — or a mismatched remainder stylo
    /// decomposes has no reach.
    #[test]
    fn unmodeled_ops_and_decomposed_remainders_have_no_reach() {
        let matrix =
            GenericTransformOperation::Matrix(stylo::values::generics::transform::Matrix {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 1.0,
                e: 10.0,
                f: 0.0,
            });
        let skew = GenericTransformOperation::SkewX(Angle::from_degrees(10.0));
        let tilt = GenericTransformOperation::RotateX(Angle::from_degrees(10.0));
        for (from, to) in [
            (vec![translate_x(0.0)], vec![rotate(90.0)]),
            (vec![matrix.clone()], vec![matrix]),
            (vec![skew.clone()], vec![skew]),
            (vec![tilt.clone()], vec![tilt]),
        ] {
            let reach = reach(&[(list(from), list(to), LINEAR)]);
            assert!(!reach.is_bounded());
            assert!(reach.pull_back(Rect::new(0.0, 0.0, 1.0, 1.0)).is_none());
            assert!(reach.inverse_norm().is_none());
        }
    }

    /// `translate(100px, 0) rotate(90deg)` rotates first and translates
    /// last, so a pullback undoes the translation first.
    #[test]
    fn a_pullback_undoes_the_last_applied_op_first() {
        let turned = || list(vec![translate(100.0, 0.0), rotate(90.0)]);
        let reach = reach(&[(turned(), turned(), LINEAR)]);
        // L(50, 0) = translate(rotate(50, 0)) = (0, 50) + (100, 0).
        let region = Rect::new(99.0, 49.0, 101.0, 51.0);
        assert_rect(
            reach.pull_back(region).expect("bounded"),
            Rect::new(49.0, -1.0, 51.0, 1.0),
        );
    }

    /// A sweep over 30°..120° carries a point through the +y axis, whose
    /// extreme neither end of the arc reaches.
    #[test]
    fn a_rotation_crossing_an_axis_reaches_the_axis_extreme() {
        let reach = reach(&[(
            list(vec![rotate(-120.0)]),
            list(vec![rotate(-30.0)]),
            LINEAR,
        )]);
        // Undoing −120°..−30° rotates by 30°..120°: (10, 0) passes (0, 10).
        let pulled = reach
            .pull_back(Rect::new(10.0, 0.0, 10.0, 0.0))
            .expect("bounded");
        let (sin30, cos30) = 30_f64.to_radians().sin_cos();
        assert_rect(
            pulled,
            Rect::new(-10.0 * sin30, 10.0 * sin30, 10.0 * cos30, 10.0),
        );
    }

    /// A full turn or more is the bounding square of each corner's circle.
    #[test]
    fn a_full_turn_sweeps_the_whole_circle() {
        let reach = reach(&[(list(vec![rotate(0.0)]), list(vec![rotate(360.0)]), LINEAR)]);
        assert_rect(
            reach
                .pull_back(Rect::new(3.0, 4.0, 3.0, 4.0))
                .expect("bounded"),
            Rect::new(-5.0, -5.0, 5.0, 5.0),
        );
    }

    #[test]
    fn a_scale_range_divides_the_region_at_both_ends() {
        let reach = reach(&[(
            list(vec![scale(0.5, 1.0)]),
            list(vec![scale(2.0, 1.0)]),
            LINEAR,
        )]);
        assert_rect(
            reach
                .pull_back(Rect::new(-10.0, -10.0, 40.0, 10.0))
                .expect("bounded"),
            Rect::new(-20.0, -10.0, 80.0, 10.0),
        );
    }

    #[test]
    fn a_scale_range_reaching_zero_is_unbounded() {
        let region = Rect::new(0.0, 0.0, 10.0, 10.0);
        let scale_y = |y| GenericTransformOperation::ScaleY(y);
        for (from, to) in [(0.0, 1.0), (-1.0, 1.0)] {
            let reach = reach(&[(list(vec![scale_y(from)]), list(vec![scale_y(to)]), LINEAR)]);
            assert!(
                reach.is_bounded()
                    && reach.pull_back(region).is_none()
                    && reach.inverse_norm().is_none(),
                "scaleY({from}) to scaleY({to})",
            );
        }
        // An overshooting ease carries `0.1 → 1` below 0 as well.
        assert!(
            reach(&[(
                list(vec![scale(0.1, 0.1)]),
                list(vec![scale(1.0, 1.0)]),
                Interval {
                    low: -0.5,
                    high: 1.0
                },
            )])
            .pull_back(region)
            .is_none()
        );
    }

    /// `pre` and `Lc` conjugate the op ranges: a quarter-turn curve about an
    /// origin at (50, 50), committed at 0°, reaches a region about that
    /// origin only.
    #[test]
    fn the_fixed_maps_conjugate_the_op_ranges() {
        let reach = reach_of(
            &[(list(vec![rotate(0.0)]), list(vec![rotate(90.0)]), LINEAR)],
            Affine::translate((50.0, 50.0)),
            Affine::IDENTITY,
        );
        // Content at (50 + 10·cos a, 50 + 10·sin a) for a in 0..90° is
        // carried onto (50, 60) at some instant.
        assert_rect(
            reach
                .pull_back(Rect::new(50.0, 60.0, 50.0, 60.0))
                .expect("bounded"),
            Rect::new(50.0, 50.0, 60.0, 60.0),
        );
    }

    /// Content baked mid-curve at `Lc = scale(1.5)` sits at `L(t)·Lc⁻¹` of
    /// its baked place, so the pullback lifts through `Lc`: over `s ∈ [1, 2]`,
    /// `x·s/1.5 ∈ [10, 30]` holds for `x ∈ [7.5, 45]`.
    #[test]
    fn a_curve_committed_mid_scale_lifts_through_the_committed_scale() {
        let reach = reach_of(
            &[(
                list(vec![scale(1.0, 1.0)]),
                list(vec![scale(2.0, 2.0)]),
                LINEAR,
            )],
            Affine::IDENTITY,
            Affine::scale(1.5),
        );
        assert_rect(
            reach
                .pull_back(Rect::new(10.0, 10.0, 30.0, 30.0))
                .expect("bounded"),
            Rect::new(7.5, 7.5, 45.0, 45.0),
        );
    }

    /// An angle too large to add a heading to still sweeps in bounded time:
    /// a constant rotation reaches its own rotation of the region alone.
    #[test]
    fn a_huge_constant_rotation_reaches_its_own_rotation() {
        let region = Rect::new(3.0, -4.0, 6.0, 8.0);
        for degrees in [1e18_f32, 1e20, -1e20, 3.6e22] {
            let turned = || list(vec![rotate(degrees)]);
            let reach = reach(&[(turned(), turned(), LINEAR)]);
            assert_rect(
                reach.pull_back(region).expect("bounded"),
                Affine::rotate(-f64::from(degrees).to_radians()).transform_rect_bbox(region),
            );
        }
    }

    /// Whole turns before the sweep change nothing: 30°..120° after ten
    /// thousand turns still passes the +y axis.
    #[test]
    fn a_sweep_after_many_turns_reaches_the_axis_it_passes() {
        let turns = 10_000.0 * 360.0;
        let reach = reach(&[(
            list(vec![rotate(-turns - 120.0)]),
            list(vec![rotate(-turns - 30.0)]),
            LINEAR,
        )]);
        let pulled = reach
            .pull_back(Rect::new(10.0, 0.0, 10.0, 0.0))
            .expect("bounded");
        let (sin30, cos30) = 30_f64.to_radians().sin_cos();
        assert_rect(
            pulled,
            Rect::new(-10.0 * sin30, 10.0 * sin30, 10.0 * cos30, 10.0),
        );
    }

    /// A pop-in from `scale(0)` has no pullback, yet carries content forward
    /// into the hull of its box and the origin.
    #[test]
    fn a_scale_through_zero_still_carries_forward() {
        let reach = reach(&[(
            list(vec![scale(0.0, 0.0)]),
            list(vec![scale(1.0, 1.0)]),
            LINEAR,
        )]);
        assert!(reach.pull_back(Rect::new(0.0, 0.0, 1.0, 1.0)).is_none());
        assert_rect(
            reach.carry(Rect::new(10.0, -20.0, 30.0, 40.0)),
            Rect::new(0.0, -20.0, 30.0, 40.0),
        );
    }

    /// The exported curve of an element running `animation` inside a
    /// transformed parent, committed at 0.3 s.
    fn exported(keyframes: &str, animation: &str) -> std::sync::Arc<crate::CommittedFrame> {
        let mut doc = Doc::with_css(&format!(
            "page {{ display: flex; width: 800px; height: 600px; }}
             .parent {{ display: flex; margin: 100px; width: 300px; height: 200px;
                        transform: matrix(1.5, 0.2, -0.3, 0.8, 60, -15); }}
             .mover {{ display: flex; width: 80px; height: 40px; background-color: teal;
                       transform-origin: 20px 10px; animation: {animation}; }}
             @keyframes move {{ {keyframes} }}"
        ));
        let parent = doc.el(doc.root, "view.parent");
        doc.el(parent, "view.mover");
        let dom = &mut doc.dom;
        dom.render();
        dom.advance_animations(0.0);
        assert!(dom.advance_animations(0.3).needs_next_frame);
        dom.commit()
    }

    /// Every delta the exported curve samples keeps `bounds` inside the
    /// carry, pulls `region` back inside the pullback, and stretches no
    /// distance past the norm, nor its inverse past the inverse norm — with
    /// a parent world that neither commutes with the ops nor is rigid.
    #[test]
    fn every_sampled_delta_stays_inside_the_reach() {
        for (keyframes, animation) in [
            (
                "from { transform: translate(0px, 0px) rotate(-30deg) scale(0.5, 1.2); }
                 to { transform: translate(40px, -20px) rotate(90deg) scale(1.4, 0.8); }",
                "move 1s linear infinite",
            ),
            (
                "from { transform: translateX(-30px) scale(1.2); }
                 50% { transform: translate(20%, 10px) scale(0.6);
                       animation-timing-function: cubic-bezier(0.3, -0.6, 0.7, 1.6); }
                 to { transform: translateX(30px); }",
                "move 1s ease-in-out infinite alternate-reverse",
            ),
        ] {
            let frame = exported(keyframes, animation);
            let slot = &frame.animation_slots()[0];
            let reach = &slot
                .curve
                .transform
                .as_ref()
                .expect("a transform track")
                .reach;
            let bounds = Rect::new(-20.0, 5.0, 35.0, 50.0);
            let region = Rect::new(10.0, -30.0, 290.0, 220.0);
            let carried = reach.carry(bounds);
            let pulled = reach.pull_back(region).expect("bounded");
            let norm = reach.inverse_norm().expect("bounded");
            let inside = |rect: Rect, point: Point| {
                point.x >= rect.x0 - 1e-6
                    && point.x <= rect.x1 + 1e-6
                    && point.y >= rect.y0 - 1e-6
                    && point.y <= rect.y1 + 1e-6
            };
            let corners = |rect: Rect| {
                [
                    Point::new(rect.x0, rect.y0),
                    Point::new(rect.x1, rect.y0),
                    Point::new(rect.x0, rect.y1),
                    Point::new(rect.x1, rect.y1),
                ]
            };
            let mut values = stylo::properties::animated_properties::AnimationValueMap::default();
            for step in 0_u8..=80 {
                let t = 0.3 + f64::from(step) / 40.0;
                let delta = slot
                    .curve
                    .sample(Some(t), &frame.committed_offsets(), &mut values)
                    .delta;
                for corner in corners(bounds) {
                    assert!(
                        inside(carried, delta * corner),
                        "t = {t}: carry misses {corner:?}"
                    );
                }
                for corner in corners(region) {
                    let back = delta.inverse() * corner;
                    assert!(inside(pulled, back), "t = {t}: pullback misses {back:?}");
                }
                assert!(
                    delta.inverse().spectral_norm() <= norm + 1e-6,
                    "t = {t}: {} past {norm}",
                    delta.inverse().spectral_norm(),
                );
                assert!(
                    delta.spectral_norm() <= reach.norm() + 1e-6,
                    "t = {t}: {} past {}",
                    delta.spectral_norm(),
                    reach.norm(),
                );
            }
        }
    }

    /// Running reversed, a segment eases by its upper keyframe's function
    /// over flipped progress: `100 + (0 − 100)·y` over `y ∈ [−1, 1]` reaches
    /// 200 px, where the same curve forward never passes 100.
    #[test]
    fn a_reversed_segment_eases_by_its_upper_keyframe() {
        let keyframes = "from { transform: translateX(0px); }
                         to { transform: translateX(100px);
                              animation-timing-function: cubic-bezier(0.5, -1, 0.5, 1); }";
        let reaches = |direction: &str| {
            let frame = exported(keyframes, &format!("move 1s linear infinite {direction}"));
            let track = frame.animation_slots()[0]
                .curve
                .transform
                .clone()
                .expect("a transform track");
            ops(&track.reach)[0].ranges[0]
        };
        assert_eq!(
            reaches("normal"),
            Interval {
                low: 0.0,
                high: 100.0
            }
        );
        assert_eq!(
            reaches("reverse"),
            Interval {
                low: 0.0,
                high: 200.0
            }
        );
    }
}
