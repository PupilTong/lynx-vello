#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    reason = "CSS/style geometry is f32 while Vello/Kurbo geometry is f64"
)]

//! Type bridges: euclid (dom visual) → kurbo, stylo colors → peniko.

use euclid::default::Transform3D;
use stylo::color::{AbsoluteColor, ColorSpace};
use stylo::properties::ComputedValues;

use crate::Size2D;
use crate::vello::kurbo::Affine;
use crate::vello::peniko::Color;

/// The affine paint transform for an item matrix, or `None` when the matrix
/// does not render (singular / degenerate — css-transforms-1 §6).
///
/// `PaintItem::transform` is a flattened 2D-projective matrix: affine except
/// for a possible perspective row contributed by an ancestor `perspective`.
/// vello transforms are affine, so a projective matrix is approximated by
/// the affine map that agrees with the true projection at three border-box
/// corners — (0,0), (w,0), (0,h) — exact everywhere when the matrix is
/// affine, and exact at those corners otherwise (recorded v1 limit; hit
/// testing in `dom` keeps the exact projective inverse).
pub(crate) fn item_affine(transform: &Transform3D<f32>, size: Size2D<f32>) -> Option<Affine> {
    if !transform.is_invertible() {
        return None;
    }
    let has_perspective =
        transform.m14 != 0.0 || transform.m24 != 0.0 || (transform.m44 - 1.0).abs() > 1e-6;
    if !has_perspective {
        return Some(Affine::new([
            transform.m11 as f64,
            transform.m12 as f64,
            transform.m21 as f64,
            transform.m22 as f64,
            transform.m41 as f64,
            transform.m42 as f64,
        ]));
    }
    // Three-corner projective fit. Degenerate projections (a corner on the
    // vanishing line) fall back to skipping the item.
    let origin = transform.transform_point2d(euclid::default::Point2D::new(0.0, 0.0))?;
    let right = transform.transform_point2d(euclid::default::Point2D::new(size.width, 0.0))?;
    let down = transform.transform_point2d(euclid::default::Point2D::new(0.0, size.height))?;
    if size.width <= 0.0 || size.height <= 0.0 {
        return Some(Affine::translate((origin.x as f64, origin.y as f64)));
    }
    let a = (right.x - origin.x) as f64 / size.width as f64;
    let b = (right.y - origin.y) as f64 / size.width as f64;
    let c = (down.x - origin.x) as f64 / size.height as f64;
    let d = (down.y - origin.y) as f64 / size.height as f64;
    Some(Affine::new([a, b, c, d, origin.x as f64, origin.y as f64]))
}

/// A stylo absolute color as a peniko sRGB color.
pub(crate) fn color(absolute: AbsoluteColor) -> Color {
    let srgb = absolute.to_color_space(ColorSpace::Srgb);
    Color::new([
        srgb.components.0,
        srgb.components.1,
        srgb.components.2,
        srgb.alpha,
    ])
}

/// Resolves a computed `<color>` (which may still reference `currentcolor`)
/// against its element's style.
pub(crate) fn resolve_color(
    style: &ComputedValues,
    value: &stylo::values::computed::Color,
) -> Color {
    color(style.resolve_color(value))
}

/// The element's used `color` (text color).
pub(crate) fn current_color(style: &ComputedValues) -> Color {
    color(style.clone_color())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::vello::kurbo::Point;

    #[test]
    fn affine_matrices_convert_exactly() {
        let transform = Transform3D::new(
            2.0, 0.5, 0.0, 0.0, //
            -0.5, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            7.0, 9.0, 0.0, 1.0,
        );
        let affine = item_affine(&transform, Size2D::new(10.0, 10.0)).unwrap();
        let expected = [2.0, 0.5, -0.5, 1.0, 7.0, 9.0];
        for (actual, wanted) in affine.as_coeffs().into_iter().zip(expected) {
            assert!((actual - wanted).abs() < 1e-12, "{actual} != {wanted}");
        }
    }

    #[test]
    fn singular_matrices_do_not_render() {
        let transform = Transform3D::scale(0.0, 1.0, 1.0);
        assert!(item_affine(&transform, Size2D::new(10.0, 10.0)).is_none());
    }

    #[test]
    fn projective_matrices_agree_at_the_fitted_corners() {
        // perspective(100px) then translate3d(0, 0, -50px): uniform shrink.
        let transform =
            Transform3D::translation(0.0, 0.0, -50.0).then(&Transform3D::perspective(100.0));
        let size = Size2D::new(80.0, 60.0);
        let affine = item_affine(&transform, size).unwrap();
        for corner in [(0.0_f32, 0.0_f32), (80.0, 0.0), (0.0, 60.0)] {
            let truth = transform
                .transform_point2d(euclid::default::Point2D::new(corner.0, corner.1))
                .unwrap();
            let approx = affine * Point::new(corner.0 as f64, corner.1 as f64);
            assert!((approx.x - truth.x as f64).abs() < 1e-4);
            assert!((approx.y - truth.y as f64).abs() < 1e-4);
        }
    }
}
