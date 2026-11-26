//! The path builder shared by `Path2D` and the context's current default
//! path (HTML "Building paths").
//!
//! Every builder step takes the transform its points go through before they
//! are stored: the identity for a `Path2D`, the current transformation
//! matrix for the default path, which the standard keeps in canvas space.

use std::f64::consts::TAU;

use super::CanvasError;
use crate::kurbo::{self, Affine, BezPath, PathEl, PathSeg, Point, Vec2};
use crate::svg::shapes::{arc_tolerance, path_data};

#[derive(Clone, Debug, Default)]
pub struct Path {
    pub(crate) elements: BezPath,
    current: Point,
    start: Point,
    pending: bool,
}

const ARC_TOLERANCE: f64 = 1e-3;

impl Path {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_svg(data: &str) -> Self {
        let mut path = Self::new();
        for element in path_data(data) {
            match element {
                PathEl::MoveTo(p) => path.move_to_point(p),
                PathEl::LineTo(p) => path.line_to_point(p),
                PathEl::QuadTo(c, p) => path.quad_to_points(c, p),
                PathEl::CurveTo(c1, c2, p) => path.cubic_to_points(c1, c2, p),
                PathEl::ClosePath => path.close_path(),
            }
        }
        path
    }

    pub fn add_path(&mut self, path: &Self, transform: [f64; 6]) {
        if !transform.iter().all(|value| value.is_finite()) || !path.has_subpaths() {
            return;
        }
        let transform = Affine::new(transform);
        for element in &path.elements {
            let element = transform * element;
            if let PathEl::MoveTo(p) = element {
                self.elements.push(PathEl::MoveTo(p));
            } else {
                if self.pending {
                    self.elements.push(PathEl::MoveTo(self.current));
                }
                self.elements.push(element);
            }
            self.pending = false;
        }
        let last = transform * path.current;
        self.current = last;
        self.start = last;
        self.pending = true;
    }

    pub(crate) fn has_subpaths(&self) -> bool {
        self.pending || !self.elements.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.elements.truncate(0);
        self.pending = false;
    }

    pub(crate) fn closed(&self) -> BezPath {
        let mut closed = BezPath::new();
        let mut open = false;
        for element in &self.elements {
            match element {
                PathEl::MoveTo(_) if open => closed.push(PathEl::ClosePath),
                PathEl::ClosePath => open = false,
                _ => open = true,
            }
            closed.push(element);
        }
        if open {
            closed.push(PathEl::ClosePath);
        }
        closed
    }

    fn begin_segment(&mut self) {
        if self.pending {
            self.elements.push(PathEl::MoveTo(self.current));
            self.pending = false;
        }
    }

    fn ensure_subpath(&mut self, p: Point) {
        if !self.has_subpaths() {
            self.move_to_point(p);
        }
    }

    fn move_to_point(&mut self, p: Point) {
        self.current = p;
        self.start = p;
        self.pending = true;
    }

    fn line_to_point(&mut self, p: Point) {
        if !self.has_subpaths() {
            self.move_to_point(p);
            return;
        }
        self.begin_segment();
        self.elements.push(PathEl::LineTo(p));
        self.current = p;
    }

    fn quad_to_points(&mut self, c: Point, p: Point) {
        self.ensure_subpath(c);
        self.begin_segment();
        self.elements.push(PathEl::QuadTo(c, p));
        self.current = p;
    }

    fn cubic_to_points(&mut self, c1: Point, c2: Point, p: Point) {
        self.ensure_subpath(c1);
        self.begin_segment();
        self.elements.push(PathEl::CurveTo(c1, c2, p));
        self.current = p;
    }

    pub fn close_path(&mut self) {
        if !self.has_subpaths() {
            return;
        }
        if !self.pending {
            self.elements.push(PathEl::ClosePath);
        }
        self.current = self.start;
        self.pending = true;
    }

    pub(crate) fn move_to_with(&mut self, t: Affine, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.move_to_point(t * Point::new(x, y));
        }
    }

    pub(crate) fn line_to_with(&mut self, t: Affine, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.line_to_point(t * Point::new(x, y));
        }
    }

    pub(crate) fn quadratic_curve_to_with(&mut self, t: Affine, c: [f64; 2], p: [f64; 2]) {
        if c.iter().chain(&p).all(|v| v.is_finite()) {
            self.quad_to_points(t * pt(c), t * pt(p));
        }
    }

    pub(crate) fn bezier_curve_to_with(
        &mut self,
        t: Affine,
        c1: [f64; 2],
        c2: [f64; 2],
        p: [f64; 2],
    ) {
        if c1.iter().chain(&c2).chain(&p).all(|v| v.is_finite()) {
            self.cubic_to_points(t * pt(c1), t * pt(c2), t * pt(p));
        }
    }

    pub(crate) fn rect_with(&mut self, t: Affine, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return;
        }
        let corner = |dx, dy| t * Point::new(x + dx, y + dy);
        self.move_to_point(corner(0.0, 0.0));
        self.line_to_point(corner(w, 0.0));
        self.line_to_point(corner(w, h));
        self.line_to_point(corner(0.0, h));
        self.close_path();
    }

    #[expect(clippy::too_many_arguments, reason = "the IDL method's arguments")]
    pub(crate) fn ellipse_with(
        &mut self,
        t: Affine,
        x: f64,
        y: f64,
        rx: f64,
        ry: f64,
        rotation: f64,
        start: f64,
        end: f64,
        anticlockwise: bool,
    ) -> Result<(), CanvasError> {
        if ![x, y, rx, ry, rotation, start, end]
            .iter()
            .all(|v| v.is_finite())
        {
            return Ok(());
        }
        if rx < 0.0 || ry < 0.0 {
            return Err(CanvasError::IndexSize);
        }
        let sweep = if anticlockwise {
            if start - end >= TAU {
                -TAU
            } else {
                -(start - end).rem_euclid(TAU)
            }
        } else if end - start >= TAU {
            TAU
        } else {
            (end - start).rem_euclid(TAU)
        };
        let arc = kurbo::Arc::new((x, y), (rx, ry), start, sweep, rotation);
        self.append_arc(t, &arc);
        Ok(())
    }

    fn append_arc(&mut self, t: Affine, arc: &kurbo::Arc) {
        let start = t * arc_point(arc, arc.start_angle);
        if self.has_subpaths() {
            self.line_to_point(start);
        } else {
            self.move_to_point(start);
        }
        for element in arc.append_iter(tolerance(t, arc)) {
            if let PathEl::CurveTo(c1, c2, p) = element {
                self.cubic_to_points(t * c1, t * c2, t * p);
            }
        }
    }

    pub(crate) fn arc_to_with(
        &mut self,
        t: Affine,
        p1: [f64; 2],
        p2: [f64; 2],
        radius: f64,
    ) -> Result<(), CanvasError> {
        if !p1.iter().chain(&p2).chain([&radius]).all(|v| v.is_finite()) {
            return Ok(());
        }
        let (p1, p2) = (pt(p1), pt(p2));
        self.ensure_subpath(t * p1);
        if radius < 0.0 {
            return Err(CanvasError::IndexSize);
        }
        if t.determinant() == 0.0 {
            self.line_to_point(t * p1);
            return Ok(());
        }
        let p0 = t.inverse() * self.current;
        let (d0, d2) = (p0 - p1, p2 - p1);
        if p0 == p1 || p1 == p2 || radius == 0.0 || d0.cross(d2) == 0.0 {
            self.line_to_point(t * p1);
            return Ok(());
        }
        let (u0, u2) = (d0.normalize(), d2.normalize());
        let half = u0.dot(u2).clamp(-1.0, 1.0).acos() / 2.0;
        let along = radius / half.tan();
        let center = p1 + (u0 + u2).normalize() * (radius / half.sin());
        let t0 = p1 + u0 * along;
        let t2 = p1 + u2 * along;
        let a0 = (t0 - center).atan2();
        let a2 = (t2 - center).atan2();
        let sweep = (a2 - a0 + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI;
        let arc = kurbo::Arc::new(center, (radius, radius), a0, sweep, 0.0);
        self.append_arc(t, &arc);
        Ok(())
    }

    pub(crate) fn round_rect_with(
        &mut self,
        t: Affine,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        radii: &[[f64; 2]],
    ) -> Result<(), CanvasError> {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return Ok(());
        }
        if !(1..=4).contains(&radii.len()) {
            return Err(CanvasError::Range);
        }
        if !radii.iter().flatten().all(|v| v.is_finite()) {
            return Ok(());
        }
        if radii.iter().flatten().any(|v| *v < 0.0) {
            return Err(CanvasError::Range);
        }
        let r = |i: usize| Vec2::new(radii[i][0], radii[i][1]);
        let [mut ul, mut ur, mut lr, mut ll] = match radii.len() {
            4 => [r(0), r(1), r(2), r(3)],
            3 => [r(0), r(1), r(2), r(1)],
            2 => [r(0), r(1), r(0), r(1)],
            _ => [r(0); 4],
        };
        let (aw, ah) = (w.abs(), h.abs());
        let scale = [
            aw / (ul.x + ur.x),
            ah / (ur.y + lr.y),
            aw / (lr.x + ll.x),
            ah / (ul.y + ll.y),
        ]
        .into_iter()
        .fold(1.0_f64, f64::min);
        if scale < 1.0 {
            for corner in [&mut ul, &mut ur, &mut lr, &mut ll] {
                *corner *= scale;
            }
        }
        let mut flip = Affine::IDENTITY;
        let (mut x0, mut y0) = (x, y);
        if w < 0.0 {
            x0 = x + w;
            flip = Affine::new([-1.0, 0.0, 0.0, 1.0, 2.0 * x0 + aw, 0.0]) * flip;
        }
        if h < 0.0 {
            y0 = y + h;
            flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * y0 + ah]) * flip;
        }
        let tf = t * flip;
        let at = |px: f64, py: f64| tf * Point::new(px, py);
        let (x1, y1) = (x0 + aw, y0 + ah);
        let corner = |path: &mut Self, center: Point, radii: Vec2, from: f64| {
            let arc = kurbo::Arc::new(center, radii, from, TAU / 4.0, 0.0);
            for element in arc.append_iter(tolerance(tf, &arc)) {
                if let PathEl::CurveTo(c1, c2, p) = element {
                    path.cubic_to_points(tf * c1, tf * c2, tf * p);
                }
            }
        };
        self.move_to_point(at(x0 + ul.x, y0));
        self.line_to_point(at(x1 - ur.x, y0));
        corner(self, Point::new(x1 - ur.x, y0 + ur.y), ur, -TAU / 4.0);
        self.line_to_point(at(x1, y1 - lr.y));
        corner(self, Point::new(x1 - lr.x, y1 - lr.y), lr, 0.0);
        self.line_to_point(at(x0 + ll.x, y1));
        corner(self, Point::new(x0 + ll.x, y1 - ll.y), ll, TAU / 4.0);
        self.line_to_point(at(x0, y0 + ul.y));
        corner(self, Point::new(x0 + ul.x, y0 + ul.y), ul, TAU / 2.0);
        self.close_path();
        self.move_to_point(t * Point::new(x, y));
        Ok(())
    }
}

fn tolerance(t: Affine, arc: &kurbo::Arc) -> f64 {
    arc_tolerance(arc, ARC_TOLERANCE / scale_of(t).max(f64::MIN_POSITIVE))
}

fn arc_point(arc: &kurbo::Arc, angle: f64) -> Point {
    let (sin, cos) = angle.sin_cos();
    let local = Vec2::new(arc.radii.x * cos, arc.radii.y * sin);
    arc.center + (Affine::rotate(arc.x_rotation) * local.to_point()).to_vec2()
}

fn pt(p: [f64; 2]) -> Point {
    Point::new(p[0], p[1])
}

pub(super) fn scale_of(t: Affine) -> f64 {
    let [a, b, c, d, _, _] = t.as_coeffs();
    (a * a + b * b).max(c * c + d * d).sqrt()
}

macro_rules! builder_methods {
    ($ty:ty, $transform:expr, $path:ident) => {
        impl $ty {
            pub fn move_to(&mut self, x: f64, y: f64) {
                let t = ($transform)(&*self);
                self.$path().move_to_with(t, x, y);
            }

            pub fn line_to(&mut self, x: f64, y: f64) {
                let t = ($transform)(&*self);
                self.$path().line_to_with(t, x, y);
            }

            pub fn quadratic_curve_to(&mut self, cpx: f64, cpy: f64, x: f64, y: f64) {
                let t = ($transform)(&*self);
                self.$path().quadratic_curve_to_with(t, [cpx, cpy], [x, y]);
            }

            pub fn bezier_curve_to(
                &mut self,
                cp1x: f64,
                cp1y: f64,
                cp2x: f64,
                cp2y: f64,
                x: f64,
                y: f64,
            ) {
                let t = ($transform)(&*self);
                self.$path()
                    .bezier_curve_to_with(t, [cp1x, cp1y], [cp2x, cp2y], [x, y]);
            }

            pub fn arc(
                &mut self,
                x: f64,
                y: f64,
                radius: f64,
                start_angle: f64,
                end_angle: f64,
                anticlockwise: bool,
            ) -> Result<(), CanvasError> {
                let t = ($transform)(&*self);
                self.$path().ellipse_with(
                    t,
                    x,
                    y,
                    radius,
                    radius,
                    0.0,
                    start_angle,
                    end_angle,
                    anticlockwise,
                )
            }

            pub fn arc_to(
                &mut self,
                x1: f64,
                y1: f64,
                x2: f64,
                y2: f64,
                radius: f64,
            ) -> Result<(), CanvasError> {
                let t = ($transform)(&*self);
                self.$path().arc_to_with(t, [x1, y1], [x2, y2], radius)
            }

            #[expect(clippy::too_many_arguments, reason = "the IDL method's arguments")]
            pub fn ellipse(
                &mut self,
                x: f64,
                y: f64,
                radius_x: f64,
                radius_y: f64,
                rotation: f64,
                start_angle: f64,
                end_angle: f64,
                anticlockwise: bool,
            ) -> Result<(), CanvasError> {
                let t = ($transform)(&*self);
                self.$path().ellipse_with(
                    t,
                    x,
                    y,
                    radius_x,
                    radius_y,
                    rotation,
                    start_angle,
                    end_angle,
                    anticlockwise,
                )
            }

            pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
                let t = ($transform)(&*self);
                self.$path().rect_with(t, x, y, w, h);
            }

            pub fn round_rect(
                &mut self,
                x: f64,
                y: f64,
                w: f64,
                h: f64,
                radii: &[[f64; 2]],
            ) -> Result<(), CanvasError> {
                let t = ($transform)(&*self);
                self.$path().round_rect_with(t, x, y, w, h, radii)
            }
        }
    };
}

pub(crate) use builder_methods;

impl Path {
    fn own(&mut self) -> &mut Self {
        self
    }
}

builder_methods!(Path, |_: &Path| Affine::IDENTITY, own);

pub(super) fn length(path: impl IntoIterator<Item = PathEl>) -> f64 {
    kurbo::segments(path)
        .map(|segment| match segment {
            PathSeg::Line(line) => line.p0.distance(line.p1),
            PathSeg::Quad(quad) => {
                let polygon = quad.p0.distance(quad.p1) + quad.p1.distance(quad.p2);
                f64::midpoint(quad.p0.distance(quad.p2), polygon)
            }
            PathSeg::Cubic(cubic) => {
                let polygon = cubic.p0.distance(cubic.p1)
                    + cubic.p1.distance(cubic.p2)
                    + cubic.p2.distance(cubic.p3);
                f64::midpoint(cubic.p0.distance(cubic.p3), polygon)
            }
        })
        .sum()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::f64::consts::TAU;

    use super::Path;
    use crate::kurbo::{Affine, PathEl};

    fn cubics(path: &Path) -> usize {
        path.elements
            .iter()
            .filter(|element| matches!(element, PathEl::CurveTo(..)))
            .count()
    }

    #[test]
    fn an_arc_is_a_bounded_number_of_cubics() {
        let mut path = Path::new();
        path.arc(0.0, 0.0, 1e36, 0.0, TAU, false).unwrap();
        assert!(cubics(&path) <= 33, "{}", cubics(&path));

        for (scale, radius) in [(1e200, 1.0), (1.0, 1e300), (1e300, 1e300), (1e-300, 1.0)] {
            let t = Affine::scale(scale);
            let mut path = Path::new();
            path.ellipse_with(t, 0.0, 0.0, radius, radius, 0.0, 0.0, TAU, false)
                .unwrap();
            assert!(cubics(&path) <= 33, "{scale} {radius}: {}", cubics(&path));
            let mut path = Path::new();
            path.round_rect_with(t, 0.0, 0.0, radius, radius, &[[radius / 2.0; 2]])
                .unwrap();
            assert!(
                cubics(&path) <= 4 * 9,
                "{scale} {radius}: {}",
                cubics(&path)
            );
            let mut path = Path::new();
            path.move_to_with(t, 0.0, 0.0);
            path.arc_to_with(t, [radius, 0.0], [radius, radius], radius)
                .unwrap();
            assert!(cubics(&path) <= 9, "{scale} {radius}: {}", cubics(&path));
        }
        let svg = Path::from_svg("M0 0 A1e300 1e300 0 1 1 1e300 0");
        assert!(cubics(&svg) <= 33, "{}", cubics(&svg));
    }

    #[test]
    fn an_ordinary_arc_keeps_its_tolerance() {
        let mut path = Path::new();
        path.arc(0.0, 0.0, 10.0, 0.0, TAU, false).unwrap();
        let expected = crate::kurbo::Arc::new((0.0, 0.0), (10.0, 10.0), 0.0, TAU, 0.0)
            .append_iter(1e-3)
            .count();
        assert_eq!(cubics(&path), expected);
    }
}
