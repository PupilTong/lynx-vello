//! Geometry: path data and the basic shapes as kurbo paths.

use svgtypes::{PathParser, PathSegment, PointsParser};

use crate::vello::kurbo::{Arc, BezPath, Circle, Ellipse, Point, Rect, Shape, SvgArc, Vec2};

/// The tolerance arcs (elliptical arcs, circles, rounded corners) are
/// turned into cubic Béziers at, in user units. Small: a 24-unit icon is
/// routinely drawn at ten times its size.
pub(super) const ARC_TOLERANCE: f64 = 0.01;

/// Path data as a kurbo path. A malformed segment ends the path there,
/// keeping everything before it, as the SVG specification asks.
///
/// Two rules shape the output: a segment after `Z` that is not a `MoveTo`
/// starts at the closed subpath's first point, so the conversion moves
/// there first (`BezPath` has no implicit current point after a close);
/// and `S`/`T` reflect the previous control point only after a segment of
/// their own kind, as the specification says.
#[expect(clippy::too_many_lines, reason = "one arm per SVG path command")]
pub(super) fn path_data(data: &str) -> BezPath {
    let mut path = BezPath::new();
    let mut current = Point::ZERO;
    let mut start = Point::ZERO;
    let mut closed = false;
    // The last cubic and quadratic control points, for `S` and `T`.
    let mut last_cubic: Option<Point> = None;
    let mut last_quad: Option<Point> = None;
    for segment in PathParser::from(data) {
        let Ok(segment) = segment else {
            break;
        };
        if std::mem::take(&mut closed) && !matches!(segment, PathSegment::MoveTo { .. }) {
            path.move_to(start);
            current = start;
        }
        let at = |abs: bool, x: f64, y: f64| {
            if abs {
                Point::new(x, y)
            } else {
                Point::new(current.x + x, current.y + y)
            }
        };
        let mut next_cubic = None;
        let mut next_quad = None;
        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                current = at(abs, x, y);
                start = current;
                path.move_to(current);
            }
            PathSegment::LineTo { abs, x, y } => {
                current = at(abs, x, y);
                path.line_to(current);
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                current = Point::new(if abs { x } else { current.x + x }, current.y);
                path.line_to(current);
            }
            PathSegment::VerticalLineTo { abs, y } => {
                current = Point::new(current.x, if abs { y } else { current.y + y });
                path.line_to(current);
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (p1, p2, p3) = (at(abs, x1, y1), at(abs, x2, y2), at(abs, x, y));
                path.curve_to(p1, p2, p3);
                next_cubic = Some(p2);
                current = p3;
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                let p1 = last_cubic.map_or(current, |control| reflect(control, current));
                let (p2, p3) = (at(abs, x2, y2), at(abs, x, y));
                path.curve_to(p1, p2, p3);
                next_cubic = Some(p2);
                current = p3;
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                let (p1, p2) = (at(abs, x1, y1), at(abs, x, y));
                path.quad_to(p1, p2);
                next_quad = Some(p1);
                current = p2;
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                let p1 = last_quad.map_or(current, |control| reflect(control, current));
                let p2 = at(abs, x, y);
                path.quad_to(p1, p2);
                next_quad = Some(p1);
                current = p2;
            }
            PathSegment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                let to = at(abs, x, y);
                append_arc(
                    &mut path,
                    SvgArc {
                        from: current,
                        to,
                        radii: Vec2::new(rx.abs(), ry.abs()),
                        x_rotation: x_axis_rotation.to_radians(),
                        large_arc,
                        sweep,
                    },
                );
                current = to;
            }
            PathSegment::ClosePath { .. } => {
                path.close_path();
                current = start;
                closed = true;
            }
        }
        last_cubic = next_cubic;
        last_quad = next_quad;
    }
    path
}

/// `control` mirrored through `through`.
fn reflect(control: Point, through: Point) -> Point {
    Point::new(2.0 * through.x - control.x, 2.0 * through.y - control.y)
}

/// Appends an SVG arc as cubic Béziers, or as the straight line the
/// specification substitutes when the arc is degenerate (coincident
/// endpoints draw nothing, a zero radius a line).
fn append_arc(path: &mut BezPath, arc: SvgArc) {
    if arc.from == arc.to {
        return;
    }
    match Arc::from_svg_arc(&arc) {
        Some(arc) => {
            for element in arc.append_iter(ARC_TOLERANCE) {
                path.push(element);
            }
        }
        None => path.line_to(arc.to),
    }
}

/// A `rect`: `rx`/`ry` complete each other when one is missing and are
/// clamped to half the side. Nothing for a non-positive side.
pub(super) fn rect(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    rx: Option<f64>,
    ry: Option<f64>,
) -> Option<BezPath> {
    if !(width > 0.0 && height > 0.0) {
        return None;
    }
    let rect = Rect::new(x, y, x + width, y + height);
    let (rx, ry) = match (rx, ry) {
        (None, None) => (0.0, 0.0),
        (Some(rx), None) => (rx, rx),
        (None, Some(ry)) => (ry, ry),
        (Some(rx), Some(ry)) => (rx, ry),
    };
    let rx = rx.clamp(0.0, width / 2.0);
    let ry = ry.clamp(0.0, height / 2.0);
    if !(rx > 0.0 && ry > 0.0) {
        return Some(rect.to_path(ARC_TOLERANCE));
    }
    // Four elliptical corner arcs, clockwise from the top edge's start, so
    // an `rx` unequal to `ry` is drawn exactly.
    let (x0, y0, x1, y1) = (rect.x0, rect.y0, rect.x1, rect.y1);
    let corner = |path: &mut BezPath, from: Point, to: Point| {
        append_arc(
            path,
            SvgArc {
                from,
                to,
                radii: Vec2::new(rx, ry),
                x_rotation: 0.0,
                large_arc: false,
                sweep: true,
            },
        );
    };
    let mut path = BezPath::new();
    path.move_to((x0 + rx, y0));
    path.line_to((x1 - rx, y0));
    corner(&mut path, Point::new(x1 - rx, y0), Point::new(x1, y0 + ry));
    path.line_to((x1, y1 - ry));
    corner(&mut path, Point::new(x1, y1 - ry), Point::new(x1 - rx, y1));
    path.line_to((x0 + rx, y1));
    corner(&mut path, Point::new(x0 + rx, y1), Point::new(x0, y1 - ry));
    path.line_to((x0, y0 + ry));
    corner(&mut path, Point::new(x0, y0 + ry), Point::new(x0 + rx, y0));
    path.close_path();
    Some(path)
}

/// A `circle`; nothing for a non-positive radius.
pub(super) fn circle(cx: f64, cy: f64, r: f64) -> Option<BezPath> {
    (r > 0.0).then(|| Circle::new((cx, cy), r).to_path(ARC_TOLERANCE))
}

/// An `ellipse`; a missing radius takes the other (SVG 2 `auto`), and a
/// non-positive one draws nothing.
pub(super) fn ellipse(cx: f64, cy: f64, rx: Option<f64>, ry: Option<f64>) -> Option<BezPath> {
    let (rx, ry) = match (rx, ry) {
        (Some(rx), Some(ry)) => (rx, ry),
        (Some(r), None) | (None, Some(r)) => (r, r),
        (None, None) => return None,
    };
    (rx > 0.0 && ry > 0.0).then(|| Ellipse::new((cx, cy), (rx, ry), 0.0).to_path(ARC_TOLERANCE))
}

/// A `line`.
pub(super) fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((x1, y1));
    path.line_to((x2, y2));
    path
}

/// A `polyline`, or a `polygon` when `close`. An odd coordinate count
/// drops the last number, as the specification's error handling says.
pub(super) fn polyline(points: &str, close: bool) -> BezPath {
    let mut path = BezPath::new();
    for (index, (x, y)) in PointsParser::from(points).enumerate() {
        if index == 0 {
            path.move_to((x, y));
        } else {
            path.line_to((x, y));
        }
    }
    if close && !path.is_empty() {
        path.close_path();
    }
    path
}
