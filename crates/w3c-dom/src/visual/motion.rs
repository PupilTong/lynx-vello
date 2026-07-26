//! CSS Motion Path (motion-1): the offset transform.
//!
//! `offset-path` positions an element's anchor point on a path and
//! optionally rotates it to follow the path direction. This module samples
//! the used path — the fork parses `path()`, `circle()`, `ellipse()`, and
//! `inset()` with the coord box fixed to the border box — into a flattened
//! contour (polyline with cumulative arc lengths) and answers "point and
//! direction at the used offset distance".
//!
//! Per motion-1 §2.7 the offset transform is a translation aligning the
//! anchor point with the offset position, followed by an `offset-rotate`
//! rotation. `offset-anchor` is not compiled in this build, so the anchor is
//! always `transform-origin`; `offset-position` is not compiled either, so
//! shape positions take their css-shapes defaults.
//!
//! Used distance (motion-1 §2.2): percentages resolve against the total
//! path length; closed loops (every basic shape, and SVG paths whose final
//! command is a closepath) take the distance modulo the length, open paths
//! clamp to it, and a zero-length path pins the distance to 0.

use euclid::default::{Point2D, Size2D, Vector2D};
use stylo::properties::ComputedValues;
use stylo::values::computed::CSSPixelLength;
use stylo::values::computed::basic_shape::{BasicShape, InsetRect, ShapeRadius};
use stylo::values::computed::motion::OffsetRotate;
use stylo::values::computed::position::Position as ShapePosition;
use stylo::values::generics::basic_shape::{
    ArcSize, ArcSweep, CommandEndPoint, ControlPoint, GenericBasicShape, GenericPathOrShapeFunction,
};
use stylo::values::generics::motion::{GenericOffsetPath, GenericOffsetPathFunction};
use stylo::values::generics::position::GenericPositionOrAuto;
use stylo::values::specified::svg_path::{PathCommand, SVGPathPosition};

use super::CornerRadii;
use super::geometry::normalize_corner_radii;

/// The sampled offset: the position on the path in border-box coordinates
/// and the total rotation in radians (path direction where `offset-rotate`
/// says `auto`/`reverse`, plus any fixed angle).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OffsetSample {
    pub position: Point2D<f32>,
    pub angle: f32,
}

/// Samples the style's motion path, if any. Returns `None` for
/// `offset-path: none` (and for the value shapes the lynx grammar cannot
/// produce: `ray()`, `url()`, bare coord-box).
pub(crate) fn offset_sample(
    style: &ComputedValues,
    border_box: Size2D<f32>,
) -> Option<OffsetSample> {
    let path = &style.get_box().offset_path;
    let function = match path {
        // A bare coord-box is not parseable under the lynx grammar (the
        // coord box is always the border box and a path function is
        // mandatory); treated as absent rather than guessed at.
        GenericOffsetPath::None | GenericOffsetPath::CoordBox(_) => return None,
        GenericOffsetPath::OffsetPath { path, .. } => &**path,
    };
    let contour = match function {
        // ray() and url() are gated out of the lynx parser.
        GenericOffsetPathFunction::Ray(_) | GenericOffsetPathFunction::Url(_) => return None,
        GenericOffsetPathFunction::Shape(shape) => contour_of_shape(shape, border_box)?,
    };

    let total = contour.total_length();
    let distance = style
        .clone_offset_distance()
        .resolve(CSSPixelLength::new(total))
        .px();
    let used_distance = if total <= 0.0 {
        0.0
    } else if contour.closed {
        distance.rem_euclid(total)
    } else {
        distance.clamp(0.0, total)
    };

    let (position, direction) = contour.sample(used_distance);
    let rotate: OffsetRotate = style.clone_offset_rotate();
    let angle = if rotate.auto {
        direction + rotate.angle.radians()
    } else {
        rotate.angle.radians()
    };
    Some(OffsetSample { position, angle })
}

/// Maximum deviation between a curve and its flattened polyline, in CSS px.
const FLATTEN_TOLERANCE: f32 = 0.1;

/// A flattened path: polyline vertices with cumulative arc lengths.
/// Subpath jumps (`M` inside a path) contribute segments of zero length —
/// the walk teleports, matching "total length of all sub-paths".
struct Contour {
    points: Vec<Point2D<f32>>,
    /// `cumulative[i]` = arc length from the start to `points[i]`.
    cumulative: Vec<f32>,
    /// Segments the sampler must skip when deriving direction: the
    /// teleporting jump between subpaths.
    jump_ends: Vec<usize>,
    closed: bool,
}

impl Contour {
    fn total_length(&self) -> f32 {
        self.cumulative.last().copied().unwrap_or(0.0)
    }

    /// Point and direction (radians, +X axis reference, y-down clockwise
    /// positive) at `distance` along the contour. The direction at a vertex
    /// is the direction of the following real segment (the SVG direction of
    /// the start of a segment); past the end it is the last real segment's.
    fn sample(&self, distance: f32) -> (Point2D<f32>, f32) {
        if self.points.len() < 2 {
            let point = self.points.first().copied().unwrap_or(Point2D::origin());
            return (point, 0.0);
        }
        let last = self.points.len() - 1;
        // First segment whose end reaches the distance.
        let mut segment_end = match self
            .cumulative
            .binary_search_by(|length| length.partial_cmp(&distance).expect("finite arc lengths"))
        {
            Ok(exact) => exact.max(1),
            Err(insertion) => insertion.min(last),
        };
        // A distance landing exactly on a vertex takes the following
        // segment's direction; zero-length and jump segments never provide
        // a direction of their own.
        loop {
            let start = self.cumulative[segment_end - 1];
            let end = self.cumulative[segment_end];
            let is_jump = self.jump_ends.contains(&segment_end);
            if segment_end == last || (end > start && !is_jump && distance < end) {
                let span = end - start;
                let fraction = if span > 0.0 {
                    ((distance - start) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let from = self.points[segment_end - 1];
                let to = self.points[segment_end];
                let position = from + (to - from) * fraction;
                let direction = self.direction_at(segment_end);
                return (position, direction);
            }
            segment_end += 1;
        }
    }

    /// Direction of segment `end_index` (from `points[end - 1]` to
    /// `points[end]`), falling back to the nearest earlier real segment for
    /// degenerate ones.
    fn direction_at(&self, end_index: usize) -> f32 {
        let mut index = end_index;
        loop {
            let vector: Vector2D<f32> = self.points[index] - self.points[index - 1];
            if (vector.x != 0.0 || vector.y != 0.0) && !self.jump_ends.contains(&index) {
                return vector.y.atan2(vector.x);
            }
            if index == 1 {
                return 0.0;
            }
            index -= 1;
        }
    }
}

struct ContourBuilder {
    points: Vec<Point2D<f32>>,
    cumulative: Vec<f32>,
    jump_ends: Vec<usize>,
}

impl ContourBuilder {
    fn new() -> Self {
        Self {
            points: Vec::new(),
            cumulative: Vec::new(),
            jump_ends: Vec::new(),
        }
    }

    fn move_to(&mut self, point: Point2D<f32>) {
        if self.points.is_empty() {
            self.points.push(point);
            self.cumulative.push(0.0);
        } else {
            // Subpath jump: zero length, never a direction source.
            let length = self.total();
            self.points.push(point);
            self.cumulative.push(length);
            self.jump_ends.push(self.points.len() - 1);
        }
    }

    fn line_to(&mut self, point: Point2D<f32>) {
        let Some(&from) = self.points.last() else {
            return self.move_to(point);
        };
        let length = self.total() + (point - from).length();
        self.points.push(point);
        self.cumulative.push(length);
    }

    fn total(&self) -> f32 {
        self.cumulative.last().copied().unwrap_or(0.0)
    }

    fn current(&self) -> Point2D<f32> {
        self.points.last().copied().unwrap_or(Point2D::origin())
    }

    /// Adaptively flattens a cubic Bézier by recursive subdivision until the
    /// control points are within tolerance of the chord.
    fn cubic_to(
        &mut self,
        control1: Point2D<f32>,
        control2: Point2D<f32>,
        to: Point2D<f32>,
        depth: u8,
    ) {
        let from = self.current();
        let flat_enough = {
            let d1 = deviation(from, to, control1);
            let d2 = deviation(from, to, control2);
            d1 <= FLATTEN_TOLERANCE && d2 <= FLATTEN_TOLERANCE
        };
        if flat_enough || depth == 0 {
            self.line_to(to);
            return;
        }
        // de Casteljau split at t = 1/2.
        let mid = |a: Point2D<f32>, b: Point2D<f32>| {
            Point2D::new(f32::midpoint(a.x, b.x), f32::midpoint(a.y, b.y))
        };
        let ab = mid(from, control1);
        let bc = mid(control1, control2);
        let cd = mid(control2, to);
        let abc = mid(ab, bc);
        let bcd = mid(bc, cd);
        let split = mid(abc, bcd);
        self.cubic_to(ab, abc, split, depth - 1);
        self.cubic_to(bcd, cd, to, depth - 1);
    }

    /// SVG elliptical arc (endpoint parameterization, SVG 2 §B.2.4) flattened
    /// by uniform angle steps within tolerance.
    #[allow(
        clippy::too_many_arguments,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn arc_to(
        &mut self,
        mut rx: f32,
        mut ry: f32,
        x_rotation_degrees: f32,
        large_arc: bool,
        sweep_clockwise: bool,
        to: Point2D<f32>,
    ) {
        let from = self.current();
        if from == to {
            return;
        }
        rx = rx.abs();
        ry = ry.abs();
        if rx == 0.0 || ry == 0.0 {
            return self.line_to(to);
        }
        let phi = x_rotation_degrees.to_radians();
        let (sin_phi, cos_phi) = phi.sin_cos();
        // F.6.5.1: midpoint transform.
        let dx = f32::midpoint(from.x, -to.x);
        let dy = f32::midpoint(from.y, -to.y);
        let x1p = cos_phi * dx + sin_phi * dy;
        let y1p = -sin_phi * dx + cos_phi * dy;
        // F.6.6: radii scale-up when too small.
        let lambda = (x1p / rx).powi(2) + (y1p / ry).powi(2);
        if lambda > 1.0 {
            let scale = lambda.sqrt();
            rx *= scale;
            ry *= scale;
        }
        // F.6.5.2: center in the prime frame.
        let sign = if large_arc == sweep_clockwise {
            -1.0
        } else {
            1.0
        };
        let numerator = (rx * ry).powi(2) - (rx * y1p).powi(2) - (ry * x1p).powi(2);
        let denominator = (rx * y1p).powi(2) + (ry * x1p).powi(2);
        let coefficient = sign * (numerator / denominator).max(0.0).sqrt();
        let cxp = coefficient * rx * y1p / ry;
        let cyp = -coefficient * ry * x1p / rx;
        // F.6.5.3: center and angles.
        let cx = cos_phi * cxp - sin_phi * cyp + f32::midpoint(from.x, to.x);
        let cy = sin_phi * cxp + cos_phi * cyp + f32::midpoint(from.y, to.y);
        let start_angle = (y1p - cyp).atan2(x1p - cxp);
        let end_angle = (-y1p - cyp).atan2(-x1p - cxp);
        let mut delta = end_angle - start_angle;
        if sweep_clockwise && delta < 0.0 {
            delta += std::f32::consts::TAU;
        } else if !sweep_clockwise && delta > 0.0 {
            delta -= std::f32::consts::TAU;
        }

        let radius = rx.max(ry).max(f32::EPSILON);
        let max_step = 2.0 * (1.0 - FLATTEN_TOLERANCE / radius).clamp(-1.0, 1.0).acos();
        let steps = (delta.abs() / max_step.max(1e-3)).ceil().max(1.0) as u32;
        for step in 1..=steps {
            let angle = start_angle + delta * (step as f32 / steps as f32);
            let (sin_angle, cos_angle) = angle.sin_cos();
            let x = cx + rx * cos_angle * cos_phi - ry * sin_angle * sin_phi;
            let y = cy + rx * cos_angle * sin_phi + ry * sin_angle * cos_phi;
            self.line_to(Point2D::new(x, y));
        }
        // Land exactly on the endpoint despite floating-point drift.
        self.line_to(to);
    }

    fn finish(self, closed: bool) -> Contour {
        Contour {
            points: self.points,
            cumulative: self.cumulative,
            jump_ends: self.jump_ends,
            closed,
        }
    }
}

/// Perpendicular distance from `point` to the chord `from`→`to` (chord
/// length distance when the chord is degenerate).
fn deviation(from: Point2D<f32>, to: Point2D<f32>, point: Point2D<f32>) -> f32 {
    let chord = to - from;
    let offset = point - from;
    let chord_length = chord.length();
    if chord_length <= f32::EPSILON {
        return offset.length();
    }
    (chord.x * offset.y - chord.y * offset.x).abs() / chord_length
}

fn contour_of_shape(shape: &BasicShape, border_box: Size2D<f32>) -> Option<Contour> {
    match shape {
        GenericBasicShape::Rect(inset) => Some(contour_of_inset(inset, border_box)),
        GenericBasicShape::Circle(circle) => {
            let center = resolve_shape_position(&circle.position, border_box);
            let radius = resolve_circle_radius(&circle.radius, center, border_box);
            Some(contour_of_ellipse(center, radius, radius))
        }
        GenericBasicShape::Ellipse(ellipse) => {
            let center = resolve_shape_position(&ellipse.position, border_box);
            let rx = resolve_axis_radius(&ellipse.semiaxis_x, center.x, border_box.width);
            let ry = resolve_axis_radius(&ellipse.semiaxis_y, center.y, border_box.height);
            Some(contour_of_ellipse(center, rx, ry))
        }
        // polygon() is not in the fork's allowed shape set today, but its
        // equivalent path is trivially its vertex loop — keep it live.
        GenericBasicShape::Polygon(polygon) => {
            let mut builder = ContourBuilder::new();
            for (index, coordinate) in polygon.coordinates.iter().enumerate() {
                let point = Point2D::new(
                    coordinate
                        .0
                        .resolve(CSSPixelLength::new(border_box.width))
                        .px(),
                    coordinate
                        .1
                        .resolve(CSSPixelLength::new(border_box.height))
                        .px(),
                );
                if index == 0 {
                    builder.move_to(point);
                } else {
                    builder.line_to(point);
                }
            }
            let start = builder.points.first().copied();
            if let Some(start) = start {
                builder.line_to(start);
            }
            Some(builder.finish(true))
        }
        GenericBasicShape::PathOrShape(function) => match function {
            GenericPathOrShapeFunction::Path(path) => Some(contour_of_svg_path(&path.path)),
            // shape() is not parseable in this fork's grammar.
            GenericPathOrShapeFunction::Shape(_) => None,
        },
    }
}

fn contour_of_svg_path(path: &stylo::values::specified::svg_path::SVGPathData) -> Contour {
    let normalized = path.normalize(/* reduce = */ true);
    let mut builder = ContourBuilder::new();
    let mut closed_at_end = false;
    let mut subpath_start = Point2D::origin();

    let absolute = |point: &CommandEndPoint<SVGPathPosition, f32>| match point {
        CommandEndPoint::ToPosition(position) => {
            Point2D::new(position.horizontal, position.vertical)
        }
        CommandEndPoint::ByCoordinate(_) => {
            unreachable!("SVGPathData::normalize produces absolute endpoints")
        }
    };
    let control = |point: &ControlPoint<SVGPathPosition, f32>| match point {
        ControlPoint::Absolute(position) => Point2D::new(position.horizontal, position.vertical),
        ControlPoint::Relative(_) => {
            unreachable!("SVGPathData::normalize produces absolute control points")
        }
    };

    for command in normalized.commands() {
        closed_at_end = false;
        match command {
            PathCommand::Move { point } => {
                let point = absolute(point);
                builder.move_to(point);
                subpath_start = point;
            }
            PathCommand::Line { point } => builder.line_to(absolute(point)),
            PathCommand::CubicCurve {
                point,
                control1,
                control2,
            } => builder.cubic_to(control(control1), control(control2), absolute(point), 16),
            PathCommand::Arc {
                point,
                radii,
                arc_sweep,
                arc_size,
                rotate,
            } => {
                let ry = radii.ry.as_ref().copied().unwrap_or(radii.rx);
                builder.arc_to(
                    radii.rx,
                    ry,
                    *rotate,
                    matches!(arc_size, ArcSize::Large),
                    matches!(arc_sweep, ArcSweep::Cw),
                    absolute(point),
                );
            }
            PathCommand::Close => {
                builder.line_to(subpath_start);
                closed_at_end = true;
            }
            _ => unreachable!("SVGPathData::normalize(reduce) restricts to M, L, C, A, Z"),
        }
    }
    // Motion-1: an offset path is a closed loop only if the final command is
    // a closepath.
    builder.finish(closed_at_end)
}

/// The equivalent path of `circle()`/`ellipse()` (motion-1 §3): starts at
/// the rightmost point, proceeds clockwise (y-down), closed.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn contour_of_ellipse(center: Point2D<f32>, rx: f32, ry: f32) -> Contour {
    let mut builder = ContourBuilder::new();
    builder.move_to(Point2D::new(center.x + rx, center.y));
    if rx > 0.0 && ry > 0.0 {
        let radius = rx.max(ry);
        let max_step = 2.0 * (1.0 - FLATTEN_TOLERANCE / radius).clamp(-1.0, 1.0).acos();
        let steps = (std::f32::consts::TAU / max_step.max(1e-3)).ceil().max(8.0) as u32;
        for step in 1..=steps {
            let angle = std::f32::consts::TAU * (step as f32 / steps as f32);
            builder.line_to(Point2D::new(
                center.x + rx * angle.cos(),
                center.y + ry * angle.sin(),
            ));
        }
    }
    builder.finish(true)
}

/// The equivalent path of `inset()` (motion-1 §3): the possibly-rounded
/// rectangle outline, starting at the left end of the top straight edge,
/// proceeding clockwise, closed.
fn contour_of_inset(inset: &InsetRect, border_box: Size2D<f32>) -> Contour {
    let top = inset
        .rect
        .0
        .resolve(CSSPixelLength::new(border_box.height))
        .px();
    let right = inset
        .rect
        .1
        .resolve(CSSPixelLength::new(border_box.width))
        .px();
    let bottom = inset
        .rect
        .2
        .resolve(CSSPixelLength::new(border_box.height))
        .px();
    let left = inset
        .rect
        .3
        .resolve(CSSPixelLength::new(border_box.width))
        .px();
    let origin = Point2D::new(left, top);
    let size = Size2D::new(
        (border_box.width - left - right).max(0.0),
        (border_box.height - top - bottom).max(0.0),
    );

    let resolve_corner = |corner: &stylo::values::generics::border::BorderCornerRadius<
        stylo::values::computed::NonNegativeLengthPercentage,
    >| {
        Size2D::new(
            corner
                .0
                .width
                .0
                .resolve(CSSPixelLength::new(size.width))
                .px()
                .max(0.0),
            corner
                .0
                .height
                .0
                .resolve(CSSPixelLength::new(size.height))
                .px()
                .max(0.0),
        )
    };
    let mut radii = CornerRadii {
        top_left: resolve_corner(&inset.round.top_left),
        top_right: resolve_corner(&inset.round.top_right),
        bottom_right: resolve_corner(&inset.round.bottom_right),
        bottom_left: resolve_corner(&inset.round.bottom_left),
    };
    normalize_corner_radii(&mut radii, size);

    let max = Point2D::new(origin.x + size.width, origin.y + size.height);
    let mut builder = ContourBuilder::new();
    builder.move_to(Point2D::new(origin.x + radii.top_left.width, origin.y));
    // Top edge → top-right corner.
    builder.line_to(Point2D::new(max.x - radii.top_right.width, origin.y));
    quarter_arc(
        &mut builder,
        Point2D::new(
            max.x - radii.top_right.width,
            origin.y + radii.top_right.height,
        ),
        radii.top_right,
        -std::f32::consts::FRAC_PI_2,
    );
    // Right edge → bottom-right corner.
    builder.line_to(Point2D::new(max.x, max.y - radii.bottom_right.height));
    quarter_arc(
        &mut builder,
        Point2D::new(
            max.x - radii.bottom_right.width,
            max.y - radii.bottom_right.height,
        ),
        radii.bottom_right,
        0.0,
    );
    // Bottom edge → bottom-left corner.
    builder.line_to(Point2D::new(origin.x + radii.bottom_left.width, max.y));
    quarter_arc(
        &mut builder,
        Point2D::new(
            origin.x + radii.bottom_left.width,
            max.y - radii.bottom_left.height,
        ),
        radii.bottom_left,
        std::f32::consts::FRAC_PI_2,
    );
    // Left edge → top-left corner.
    builder.line_to(Point2D::new(origin.x, origin.y + radii.top_left.height));
    quarter_arc(
        &mut builder,
        Point2D::new(
            origin.x + radii.top_left.width,
            origin.y + radii.top_left.height,
        ),
        radii.top_left,
        std::f32::consts::PI,
    );
    builder.finish(true)
}

/// A clockwise quarter ellipse from `start_angle` (radians, y-down) around
/// `center` with the corner's radii.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn quarter_arc(
    builder: &mut ContourBuilder,
    center: Point2D<f32>,
    radius: Size2D<f32>,
    start_angle: f32,
) {
    if radius.width <= 0.0 || radius.height <= 0.0 {
        return;
    }
    let largest = radius.width.max(radius.height);
    let max_step = 2.0 * (1.0 - FLATTEN_TOLERANCE / largest).clamp(-1.0, 1.0).acos();
    let steps = ((std::f32::consts::FRAC_PI_2) / max_step.max(1e-3))
        .ceil()
        .max(2.0) as u32;
    for step in 1..=steps {
        let angle = start_angle + std::f32::consts::FRAC_PI_2 * (step as f32 / steps as f32);
        builder.line_to(Point2D::new(
            center.x + radius.width * angle.cos(),
            center.y + radius.height * angle.sin(),
        ));
    }
}

/// `at <position>` with the css-shapes default (center) when omitted —
/// `offset-position` is not compiled, so there is no starting-position
/// override.
fn resolve_shape_position(
    position: &GenericPositionOrAuto<ShapePosition>,
    border_box: Size2D<f32>,
) -> Point2D<f32> {
    match position {
        GenericPositionOrAuto::Position(position) => Point2D::new(
            position
                .horizontal
                .resolve(CSSPixelLength::new(border_box.width))
                .px(),
            position
                .vertical
                .resolve(CSSPixelLength::new(border_box.height))
                .px(),
        ),
        GenericPositionOrAuto::Auto => {
            Point2D::new(border_box.width / 2.0, border_box.height / 2.0)
        }
    }
}

/// css-shapes-1 circle radius: percentages against
/// `sqrt(w² + h²) / sqrt(2)`, `closest-side`/`farthest-side` (and the
/// corner keywords the type carries) against the reference box edges.
fn resolve_circle_radius(
    radius: &ShapeRadius,
    center: Point2D<f32>,
    border_box: Size2D<f32>,
) -> f32 {
    let side_distances = [
        center.x,
        border_box.width - center.x,
        center.y,
        border_box.height - center.y,
    ];
    match radius {
        ShapeRadius::Length(length) => {
            let basis =
                (border_box.width.powi(2) + border_box.height.powi(2)).sqrt() / 2.0_f32.sqrt();
            length.0.resolve(CSSPixelLength::new(basis)).px().max(0.0)
        }
        ShapeRadius::ClosestSide => side_distances.iter().fold(f32::MAX, |a, &b| a.min(b.abs())),
        ShapeRadius::FarthestSide => side_distances.iter().fold(0.0, |a, &b| a.max(b.abs())),
        ShapeRadius::ClosestCorner | ShapeRadius::FarthestCorner => {
            let corner = |x: f32, y: f32| ((center.x - x).powi(2) + (center.y - y).powi(2)).sqrt();
            let distances = [
                corner(0.0, 0.0),
                corner(border_box.width, 0.0),
                corner(0.0, border_box.height),
                corner(border_box.width, border_box.height),
            ];
            if matches!(radius, ShapeRadius::ClosestCorner) {
                distances.iter().fold(f32::MAX, |a, &b| a.min(b))
            } else {
                distances.iter().fold(0.0, |a, &b| a.max(b))
            }
        }
    }
}

/// css-shapes-1 ellipse semiaxis: percentages against the axis dimension,
/// side keywords against the distances along that axis.
fn resolve_axis_radius(radius: &ShapeRadius, center: f32, axis_extent: f32) -> f32 {
    match radius {
        ShapeRadius::Length(length) => length
            .0
            .resolve(CSSPixelLength::new(axis_extent))
            .px()
            .max(0.0),
        ShapeRadius::ClosestSide => center.abs().min((axis_extent - center).abs()),
        ShapeRadius::FarthestSide | ShapeRadius::ClosestCorner | ShapeRadius::FarthestCorner => {
            center.abs().max((axis_extent - center).abs())
        }
    }
}
