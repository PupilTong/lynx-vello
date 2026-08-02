#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    reason = "CSS/style geometry is f32 while Vello/Kurbo geometry is f64"
)]

//! Box geometry: rounded-rect shapes with per-corner elliptical radii, ring
//! (border/outline) paths, and `clip-path` basic-shape resolution.

use stylo::properties::ComputedValues;
use stylo::values::computed::basic_shape::{BasicShape, InsetRect};
use stylo::values::computed::{Length, LengthPercentage};
use stylo::values::generics::basic_shape::{
    GenericClipPath, GenericPathOrShapeFunction, GenericShapeRadius, ShapeBox, ShapeGeometryBox,
};
use stylo::values::generics::position::PositionOrAuto;
use stylo::values::specified::svg_path::{PathCommand, SVGPathData, SVGPathPosition};

use crate::Size2D;
use crate::layout::Edges;
use crate::vello::kurbo::{BezPath, Point, Rect, RoundedRect, RoundedRectRadii, Shape};
use crate::vello::peniko::Fill;
use crate::visual::CornerRadii;

/// Cubic-Bézier circle approximation constant.
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A box outline in the cheapest kurbo form vello can consume: plain rects
/// and circular-uniform rounded rects hit vello's native shape encodings;
/// only genuinely elliptical corners pay for a `BezPath`.
#[derive(Debug, Clone)]
pub(crate) enum BoxShape {
    Rect(Rect),
    Rounded(RoundedRect),
    Path(BezPath),
}

impl BoxShape {
    pub(crate) fn new(rect: Rect, radii: &CornerRadii) -> Self {
        if radii.is_zero() {
            return Self::Rect(rect);
        }
        let circular = [
            radii.top_left,
            radii.top_right,
            radii.bottom_right,
            radii.bottom_left,
        ]
        .iter()
        .all(|corner| (corner.width - corner.height).abs() < 1e-6);
        if circular {
            return Self::Rounded(RoundedRect::from_rect(
                rect,
                RoundedRectRadii::new(
                    radii.top_left.width as f64,
                    radii.top_right.width as f64,
                    radii.bottom_right.width as f64,
                    radii.bottom_left.width as f64,
                ),
            ));
        }
        Self::Path(rounded_rect_path(rect, radii))
    }

    /// The shape as path elements appended to `path` (used to build ring
    /// paths and merged clip shapes).
    pub(crate) fn append_to(&self, path: &mut BezPath) {
        match self {
            Self::Rect(rect) => path.extend(rect.path_elements(0.1)),
            Self::Rounded(rounded) => path.extend(rounded.path_elements(0.1)),
            Self::Path(bez) => path.extend(bez.elements().iter().copied()),
        }
    }

    pub(crate) fn bounding_box(&self) -> Rect {
        match self {
            Self::Rect(rect) => *rect,
            Self::Rounded(rounded) => rounded.rect(),
            Self::Path(bez) => bez.bounding_box(),
        }
    }
}

/// Calls `f` with the shape as `&impl Shape`, preserving vello's cheap
/// encodings per variant (a closure-based visitor because `kurbo::Shape` is
/// not dyn-safe).
macro_rules! with_shape {
    ($shape:expr, |$s:ident| $body:expr) => {
        match $shape {
            $crate::shape::BoxShape::Rect($s) => $body,
            $crate::shape::BoxShape::Rounded($s) => $body,
            $crate::shape::BoxShape::Path($s) => $body,
        }
    };
}
pub(crate) use with_shape;

/// A rounded rect with per-corner elliptical radii as a closed clockwise
/// (y-down) path. `radii` must already be overlap-normalized (the
/// `dom` visual build guarantees this for item and clip radii).
pub(crate) fn rounded_rect_path(rect: Rect, radii: &CornerRadii) -> BezPath {
    let (x0, y0, x1, y1) = (rect.x0, rect.y0, rect.x1, rect.y1);
    let tl = radii.top_left;
    let tr = radii.top_right;
    let br = radii.bottom_right;
    let bl = radii.bottom_left;
    let mut path = BezPath::new();
    path.move_to((x0 + tl.width as f64, y0));
    path.line_to((x1 - tr.width as f64, y0));
    corner(
        &mut path,
        Point::new(x1 - tr.width as f64, y0),
        Point::new(x1, y0 + tr.height as f64),
        Point::new(x1, y0),
    );
    path.line_to((x1, y1 - br.height as f64));
    corner(
        &mut path,
        Point::new(x1, y1 - br.height as f64),
        Point::new(x1 - br.width as f64, y1),
        Point::new(x1, y1),
    );
    path.line_to((x0 + bl.width as f64, y1));
    corner(
        &mut path,
        Point::new(x0 + bl.width as f64, y1),
        Point::new(x0, y1 - bl.height as f64),
        Point::new(x0, y1),
    );
    path.line_to((x0, y0 + tl.height as f64));
    corner(
        &mut path,
        Point::new(x0, y0 + tl.height as f64),
        Point::new(x0 + tl.width as f64, y0),
        Point::new(x0, y0),
    );
    path.close_path();
    path
}

/// One elliptical corner from `from` to `to` bulging toward the box corner
/// `apex`, as a kappa cubic.
fn corner(path: &mut BezPath, from: Point, to: Point, apex: Point) {
    if from == to {
        return;
    }
    let c1 = from.lerp(apex, KAPPA);
    let c2 = to.lerp(apex, KAPPA);
    path.curve_to(c1, c2, to);
}

/// `outer` minus `inner` as one even-odd path (border rings, outline rings,
/// inset-shadow fields), rebuilt into the caller's reusable buffer — the
/// hot-path form, so per-frame painting reuses one allocation.
pub(crate) fn ring_path_into(path: &mut BezPath, outer: &BoxShape, inner: &BoxShape) {
    path.truncate(0);
    outer.append_to(path);
    inner.append_to(path);
}

/// [`ring_path_into`] into a fresh path (test convenience).
#[cfg(test)]
fn ring_path(outer: &BoxShape, inner: &BoxShape) -> BezPath {
    let mut path = BezPath::new();
    ring_path_into(&mut path, outer, inner);
    path
}

/// Outer radii shrunk by the border widths, clamped at zero — the padding
/// box's radii (css-backgrounds-3 §5.2).
pub(crate) fn inner_radii(radii: &CornerRadii, border: &Edges<f32>) -> CornerRadii {
    let inset = |corner: Size2D<f32>, x: f32, y: f32| {
        Size2D::new((corner.width - x).max(0.0), (corner.height - y).max(0.0))
    };
    CornerRadii {
        top_left: inset(radii.top_left, border.left, border.top),
        top_right: inset(radii.top_right, border.right, border.top),
        bottom_right: inset(radii.bottom_right, border.right, border.bottom),
        bottom_left: inset(radii.bottom_left, border.left, border.bottom),
    }
}

/// The reference boxes a basic shape can resolve against, all in the item's
/// local (border-box-origin) space.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReferenceBoxes<'a> {
    pub border: Rect,
    pub padding: Rect,
    pub content: Rect,
    /// Outer (border-box) radii.
    pub radii: &'a CornerRadii,
    pub border_widths: &'a Edges<f32>,
    pub padding_widths: &'a Edges<f32>,
}

/// The used `clip-path` for an element, resolved to a fillable shape in the
/// element's local space. `None` means no clipping (`clip-path: none`, and —
/// recorded v1 limit — `url(…)` references, which need an SVG subsystem).
pub(crate) fn clip_path_shape(
    style: &ComputedValues,
    boxes: &ReferenceBoxes<'_>,
) -> Option<(BoxShape, Fill)> {
    let clip = style.get_svg().clip_path.clone();
    match clip {
        GenericClipPath::None | GenericClipPath::Url(_) => None,
        GenericClipPath::Box(geometry) => {
            Some((geometry_box_shape(geometry, boxes), Fill::NonZero))
        }
        GenericClipPath::Shape(shape, geometry) => {
            let reference = geometry_box_rect(geometry, boxes);
            Some(basic_shape(&shape, reference))
        }
    }
}

/// A geometry box keyword as its rounded-rect shape (css-masking-1 §3.4.1:
/// the box, with its border radii). `margin-box`/SVG boxes fall back to the
/// border box (margins are not tracked in item-local space — v1 limit).
fn geometry_box_shape(geometry: ShapeGeometryBox, boxes: &ReferenceBoxes<'_>) -> BoxShape {
    match geometry {
        ShapeGeometryBox::ShapeBox(ShapeBox::PaddingBox) => BoxShape::new(
            boxes.padding,
            &inner_radii(boxes.radii, boxes.border_widths),
        ),
        ShapeGeometryBox::ShapeBox(ShapeBox::ContentBox) => {
            let padding_radii = inner_radii(boxes.radii, boxes.border_widths);
            BoxShape::new(
                boxes.content,
                &inner_radii(&padding_radii, boxes.padding_widths),
            )
        }
        _ => BoxShape::new(boxes.border, boxes.radii),
    }
}

fn geometry_box_rect(geometry: ShapeGeometryBox, boxes: &ReferenceBoxes<'_>) -> Rect {
    match geometry {
        ShapeGeometryBox::ShapeBox(ShapeBox::PaddingBox) => boxes.padding,
        ShapeGeometryBox::ShapeBox(ShapeBox::ContentBox) => boxes.content,
        _ => boxes.border,
    }
}

/// A computed `<basic-shape>` resolved against its reference box
/// (css-shapes-1 §4).
fn basic_shape(shape: &BasicShape, reference: Rect) -> (BoxShape, Fill) {
    match shape {
        BasicShape::Rect(inset) => (inset_shape(inset, reference), Fill::NonZero),
        BasicShape::Circle(circle) => {
            let center = shape_position(&circle.position, reference);
            let radius = shape_radius(&circle.radius, center, reference, RadiusAxis::Both);
            (ellipse_shape(center, radius, radius), Fill::NonZero)
        }
        BasicShape::Ellipse(ellipse) => {
            let center = shape_position(&ellipse.position, reference);
            let rx = shape_radius(&ellipse.semiaxis_x, center, reference, RadiusAxis::X);
            let ry = shape_radius(&ellipse.semiaxis_y, center, reference, RadiusAxis::Y);
            (ellipse_shape(center, rx, ry), Fill::NonZero)
        }
        BasicShape::PathOrShape(GenericPathOrShapeFunction::Path(path)) => {
            let fill = fill_rule(path.fill);
            let mut bez = svg_path(&path.path);
            // path() coordinates are reference-box-relative.
            bez.apply_affine(crate::vello::kurbo::Affine::translate((
                reference.x0,
                reference.y0,
            )));
            (BoxShape::Path(bez), fill)
        }
        // Unreachable through the fork's grammar today (`polygon()` and
        // `shape()` are rejected at parse time); polygon is implemented so a
        // grammar rebase lights it up, shape() falls back to no clipping.
        BasicShape::Polygon(polygon) => {
            let mut bez = BezPath::new();
            let mut coords = polygon.coordinates.iter().map(|coordinate| {
                Point::new(
                    reference.x0 + resolve(&coordinate.0, reference.width()),
                    reference.y0 + resolve(&coordinate.1, reference.height()),
                )
            });
            if let Some(first) = coords.next() {
                bez.move_to(first);
                for point in coords {
                    bez.line_to(point);
                }
                bez.close_path();
            }
            (BoxShape::Path(bez), fill_rule(polygon.fill))
        }
        BasicShape::PathOrShape(GenericPathOrShapeFunction::Shape(_)) => {
            (BoxShape::Rect(reference), Fill::NonZero)
        }
    }
}

fn fill_rule(rule: stylo::values::generics::basic_shape::FillRule) -> Fill {
    if matches!(
        rule,
        stylo::values::generics::basic_shape::FillRule::Evenodd
    ) {
        Fill::EvenOdd
    } else {
        Fill::NonZero
    }
}

/// `inset(top right bottom left round …)` (css-shapes-1 §4.1.4), radii
/// overlap-normalized per css-backgrounds-3 §5.5.
fn inset_shape(inset: &InsetRect, reference: Rect) -> BoxShape {
    let top = resolve(&inset.rect.0, reference.height());
    let right = resolve(&inset.rect.1, reference.width());
    let bottom = resolve(&inset.rect.2, reference.height());
    let left = resolve(&inset.rect.3, reference.width());
    let rect = Rect::new(
        reference.x0 + left,
        reference.y0 + top,
        (reference.x1 - right).max(reference.x0 + left),
        (reference.y1 - bottom).max(reference.y0 + top),
    );
    let corner = |x: &LengthPercentage, y: &LengthPercentage| {
        Size2D::new(
            resolve(x, rect.width()) as f32,
            resolve(y, rect.height()) as f32,
        )
    };
    let radii = normalize_radii(
        CornerRadii {
            top_left: corner(
                &inset.round.top_left.0.width.0,
                &inset.round.top_left.0.height.0,
            ),
            top_right: corner(
                &inset.round.top_right.0.width.0,
                &inset.round.top_right.0.height.0,
            ),
            bottom_right: corner(
                &inset.round.bottom_right.0.width.0,
                &inset.round.bottom_right.0.height.0,
            ),
            bottom_left: corner(
                &inset.round.bottom_left.0.width.0,
                &inset.round.bottom_left.0.height.0,
            ),
        },
        rect.width() as f32,
        rect.height() as f32,
    );
    BoxShape::new(rect, &radii)
}

/// css-backgrounds-3 §5.5 overlap normalization: scale all radii by the
/// smallest ratio that keeps adjacent radii within each edge.
pub(crate) fn normalize_radii(radii: CornerRadii, width: f32, height: f32) -> CornerRadii {
    let ratio = |edge: f32, a: f32, b: f32| {
        if a + b > edge && a + b > 0.0 {
            edge / (a + b)
        } else {
            1.0
        }
    };
    let scale = ratio(width, radii.top_left.width, radii.top_right.width)
        .min(ratio(
            width,
            radii.bottom_left.width,
            radii.bottom_right.width,
        ))
        .min(ratio(
            height,
            radii.top_left.height,
            radii.bottom_left.height,
        ))
        .min(ratio(
            height,
            radii.top_right.height,
            radii.bottom_right.height,
        ))
        .clamp(0.0, 1.0);
    let scaled = |corner: Size2D<f32>| Size2D::new(corner.width * scale, corner.height * scale);
    CornerRadii {
        top_left: scaled(radii.top_left),
        top_right: scaled(radii.top_right),
        bottom_right: scaled(radii.bottom_right),
        bottom_left: scaled(radii.bottom_left),
    }
}

#[derive(Clone, Copy)]
enum RadiusAxis {
    X,
    Y,
    /// `circle()`: distances to the closest/farthest side in both axes.
    Both,
}

fn shape_position(
    position: &PositionOrAuto<stylo::values::computed::Position>,
    reference: Rect,
) -> Point {
    match position {
        PositionOrAuto::Position(position) => Point::new(
            reference.x0 + resolve(&position.horizontal, reference.width()),
            reference.y0 + resolve(&position.vertical, reference.height()),
        ),
        // `auto` is the center (css-shapes-1 §4.1.1).
        PositionOrAuto::Auto => reference.center(),
    }
}

fn shape_radius(
    radius: &GenericShapeRadius<LengthPercentage>,
    center: Point,
    reference: Rect,
    axis: RadiusAxis,
) -> f64 {
    let distances = |sides: [f64; 2]| sides;
    let x_sides = distances([
        (center.x - reference.x0).abs(),
        (reference.x1 - center.x).abs(),
    ]);
    let y_sides = distances([
        (center.y - reference.y0).abs(),
        (reference.y1 - center.y).abs(),
    ]);
    match radius {
        GenericShapeRadius::Length(length) => {
            let basis = match axis {
                RadiusAxis::X => reference.width(),
                RadiusAxis::Y => reference.height(),
                // circle() percentage basis: sqrt(w² + h²)/√2 (css-shapes-1).
                RadiusAxis::Both => {
                    (reference.width().hypot(reference.height())) / std::f64::consts::SQRT_2
                }
            };
            resolve(&length.0, basis)
        }
        GenericShapeRadius::ClosestSide => match axis {
            RadiusAxis::X => x_sides[0].min(x_sides[1]),
            RadiusAxis::Y => y_sides[0].min(y_sides[1]),
            RadiusAxis::Both => x_sides[0].min(x_sides[1]).min(y_sides[0]).min(y_sides[1]),
        },
        GenericShapeRadius::FarthestSide => match axis {
            RadiusAxis::X => x_sides[0].max(x_sides[1]),
            RadiusAxis::Y => y_sides[0].max(y_sides[1]),
            RadiusAxis::Both => x_sides[0].max(x_sides[1]).max(y_sides[0]).max(y_sides[1]),
        },
        // Corner keywords belong to radial gradients, not `circle()`/
        // `ellipse()` (css-shapes-1 §2.2 grammar) — unreachable here, but
        // the shared generic carries them: corner distance per axis.
        GenericShapeRadius::ClosestCorner => match axis {
            RadiusAxis::X => x_sides[0].min(x_sides[1]),
            RadiusAxis::Y => y_sides[0].min(y_sides[1]),
            RadiusAxis::Both => corner_distances(center, reference)
                .into_iter()
                .fold(f64::INFINITY, f64::min),
        },
        GenericShapeRadius::FarthestCorner => match axis {
            RadiusAxis::X => x_sides[0].max(x_sides[1]),
            RadiusAxis::Y => y_sides[0].max(y_sides[1]),
            RadiusAxis::Both => corner_distances(center, reference)
                .into_iter()
                .fold(0.0, f64::max),
        },
    }
}

fn corner_distances(center: Point, reference: Rect) -> [f64; 4] {
    [
        Point::new(reference.x0, reference.y0),
        Point::new(reference.x1, reference.y0),
        Point::new(reference.x0, reference.y1),
        Point::new(reference.x1, reference.y1),
    ]
    .map(|corner| center.distance(corner))
}

fn ellipse_shape(center: Point, rx: f64, ry: f64) -> BoxShape {
    BoxShape::Path(
        crate::vello::kurbo::Ellipse::new(center, (rx.max(0.0), ry.max(0.0)), 0.0).to_path(0.1),
    )
}

fn resolve(length: &LengthPercentage, basis: f64) -> f64 {
    length.resolve(Length::new(basis as f32)).px() as f64
}

/// An SVG `path()` as a kurbo path. Normalization reduces the command set
/// to absolute M/L/C/A/Z (the same contract `dom`'s motion-path build
/// relies on); arcs convert through kurbo's endpoint-parameterized arcs.
fn svg_path(data: &SVGPathData) -> BezPath {
    let normalized = data.normalize(/* reduce = */ true);
    let mut path = BezPath::new();
    let mut current = Point::ZERO;
    let mut subpath_start = Point::ZERO;

    let to_point = |position: &SVGPathPosition| -> Point {
        Point::new(position.horizontal as f64, position.vertical as f64)
    };

    for command in normalized.commands() {
        match command {
            PathCommand::Move { point } => {
                let point = end_point(point, &to_point);
                path.move_to(point);
                current = point;
                subpath_start = point;
            }
            PathCommand::Line { point } => {
                let point = end_point(point, &to_point);
                path.line_to(point);
                current = point;
            }
            PathCommand::CubicCurve {
                point,
                control1,
                control2,
            } => {
                let c1 = control_point(control1, &to_point);
                let c2 = control_point(control2, &to_point);
                let point = end_point(point, &to_point);
                path.curve_to(c1, c2, point);
                current = point;
            }
            PathCommand::Arc {
                point,
                radii,
                arc_sweep,
                arc_size,
                rotate,
            } => {
                let to = end_point(point, &to_point);
                let ry = radii.ry.as_ref().copied().unwrap_or(radii.rx);
                let arc = crate::vello::kurbo::SvgArc {
                    from: current,
                    to,
                    radii: crate::vello::kurbo::Vec2::new(radii.rx as f64, ry as f64),
                    x_rotation: (*rotate as f64).to_radians(),
                    large_arc: matches!(
                        arc_size,
                        stylo::values::generics::basic_shape::ArcSize::Large
                    ),
                    sweep: matches!(
                        arc_sweep,
                        stylo::values::generics::basic_shape::ArcSweep::Cw
                    ),
                };
                match crate::vello::kurbo::Arc::from_svg_arc(&arc) {
                    Some(arc) => path.extend(arc.append_iter(0.1)),
                    // Degenerate arc: SVG 2 §B.2.4 says draw the line.
                    None => path.line_to(to),
                }
                current = to;
            }
            PathCommand::Close => {
                path.close_path();
                current = subpath_start;
            }
            _ => unreachable!("SVGPathData::normalize(reduce) restricts to M, L, C, A, Z"),
        }
    }
    path
}

fn end_point(
    point: &stylo::values::generics::basic_shape::CommandEndPoint<SVGPathPosition, f32>,
    to_point: &impl Fn(&SVGPathPosition) -> Point,
) -> Point {
    match point {
        stylo::values::generics::basic_shape::CommandEndPoint::ToPosition(position) => {
            to_point(position)
        }
        stylo::values::generics::basic_shape::CommandEndPoint::ByCoordinate(_) => {
            unreachable!("SVGPathData::normalize produces absolute endpoints")
        }
    }
}

fn control_point(
    point: &stylo::values::generics::basic_shape::ControlPoint<SVGPathPosition, f32>,
    to_point: &impl Fn(&SVGPathPosition) -> Point,
) -> Point {
    match point {
        stylo::values::generics::basic_shape::ControlPoint::Absolute(position) => {
            to_point(position)
        }
        stylo::values::generics::basic_shape::ControlPoint::Relative(_) => {
            unreachable!("SVGPathData::normalize produces absolute control points")
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn zero_radii_stay_plain_rects() {
        let shape = BoxShape::new(Rect::new(0.0, 0.0, 10.0, 10.0), &CornerRadii::ZERO);
        assert!(matches!(shape, BoxShape::Rect(_)));
    }

    #[test]
    fn circular_radii_use_native_rounded_rects() {
        let radii = CornerRadii {
            top_left: Size2D::new(4.0, 4.0),
            top_right: Size2D::new(2.0, 2.0),
            bottom_right: Size2D::new(0.0, 0.0),
            bottom_left: Size2D::new(1.0, 1.0),
        };
        let shape = BoxShape::new(Rect::new(0.0, 0.0, 10.0, 10.0), &radii);
        assert!(matches!(shape, BoxShape::Rounded(_)));
    }

    #[test]
    fn elliptical_paths_stay_inside_the_rect_and_cover_the_center() {
        let radii = CornerRadii {
            top_left: Size2D::new(8.0, 4.0),
            top_right: Size2D::new(2.0, 6.0),
            bottom_right: Size2D::new(5.0, 5.0),
            bottom_left: Size2D::new(0.0, 0.0),
        };
        let rect = Rect::new(0.0, 0.0, 20.0, 16.0);
        let BoxShape::Path(path) = BoxShape::new(rect, &radii) else {
            panic!("elliptical radii must build a path");
        };
        let bounds = path.bounding_box();
        assert!(rect.contains(bounds.origin()));
        assert!(bounds.x1 <= rect.x1 + 1e-6 && bounds.y1 <= rect.y1 + 1e-6);
        // Winding at the center must be nonzero (the path is a closed loop).
        assert_ne!(path.winding(rect.center()), 0);
        // The sharp corner stays sharp.
        assert_eq!(path.winding(Point::new(0.5, 15.5)), 1);
    }

    #[test]
    fn ring_paths_are_empty_between_outer_and_inner() {
        let outer = BoxShape::new(Rect::new(0.0, 0.0, 10.0, 10.0), &CornerRadii::ZERO);
        let inner = BoxShape::new(Rect::new(2.0, 2.0, 8.0, 8.0), &CornerRadii::ZERO);
        let ring = ring_path(&outer, &inner);
        // Even-odd: 1 in the ring, 2 in the hole.
        assert_eq!(ring.winding(Point::new(1.0, 5.0)), 1);
        assert_eq!(ring.winding(Point::new(5.0, 5.0)), 2);
    }

    #[test]
    fn radii_normalization_scales_uniformly() {
        let radii = normalize_radii(
            CornerRadii {
                top_left: Size2D::new(30.0, 10.0),
                top_right: Size2D::new(30.0, 10.0),
                bottom_right: Size2D::new(0.0, 0.0),
                bottom_left: Size2D::new(0.0, 0.0),
            },
            40.0,
            40.0,
        );
        // 30 + 30 > 40 ⇒ scale = 40/60.
        assert!((radii.top_left.width - 20.0).abs() < 1e-5);
        assert!((radii.top_left.height - 10.0 * (40.0 / 60.0)).abs() < 1e-5);
    }
}
