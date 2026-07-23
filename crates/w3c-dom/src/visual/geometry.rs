//! Rounded-rect geometry: border-radius resolution and containment.

use euclid::default::{Point2D, Rect, Size2D};
use neutron_star::geometry::Edges;
use stylo::properties::ComputedValues;
use stylo::values::computed::CSSPixelLength;
use stylo::values::computed::border::BorderCornerRadius;

use super::CornerRadii;

/// Resolves the four `border-*-radius` values against the border-box size
/// (horizontal radii against the width, vertical against the height) and
/// applies the css-backgrounds §5.5 overlap normalization: one scale factor,
/// the minimum of `edge / (r₁ + r₂)` over the four edges, applied to every
/// radius when below 1.
pub(crate) fn resolve_corner_radii(style: &ComputedValues, size: Size2D<f32>) -> CornerRadii {
    let border = style.get_border();
    let resolve = |radius: &BorderCornerRadius| {
        Size2D::new(
            radius
                .0
                .width
                .0
                .resolve(CSSPixelLength::new(size.width))
                .px()
                .max(0.0),
            radius
                .0
                .height
                .0
                .resolve(CSSPixelLength::new(size.height))
                .px()
                .max(0.0),
        )
    };
    let mut radii = CornerRadii {
        top_left: resolve(&border.border_top_left_radius),
        top_right: resolve(&border.border_top_right_radius),
        bottom_right: resolve(&border.border_bottom_right_radius),
        bottom_left: resolve(&border.border_bottom_left_radius),
    };
    if radii.is_zero() {
        return radii;
    }

    let mut factor = 1.0_f32;
    let mut consider = |edge: f32, r1: f32, r2: f32| {
        let sum = r1 + r2;
        if sum > 0.0 {
            factor = factor.min(edge / sum);
        }
    };
    consider(size.width, radii.top_left.width, radii.top_right.width);
    consider(
        size.width,
        radii.bottom_left.width,
        radii.bottom_right.width,
    );
    consider(size.height, radii.top_left.height, radii.bottom_left.height);
    consider(
        size.height,
        radii.top_right.height,
        radii.bottom_right.height,
    );
    if factor < 1.0 {
        for corner in [
            &mut radii.top_left,
            &mut radii.top_right,
            &mut radii.bottom_right,
            &mut radii.bottom_left,
        ] {
            corner.width *= factor;
            corner.height *= factor;
        }
    }
    radii
}

/// Insets outer radii by the border widths to the padding-edge radii used
/// for overflow clipping, clamping each component at zero.
pub(crate) fn inner_radii(outer: CornerRadii, border: &Edges<f32>) -> CornerRadii {
    let inset = |corner: Size2D<f32>, horizontal: f32, vertical: f32| {
        Size2D::new(
            (corner.width - horizontal).max(0.0),
            (corner.height - vertical).max(0.0),
        )
    };
    CornerRadii {
        top_left: inset(outer.top_left, border.left, border.top),
        top_right: inset(outer.top_right, border.right, border.top),
        bottom_right: inset(outer.bottom_right, border.right, border.bottom),
        bottom_left: inset(outer.bottom_left, border.left, border.bottom),
    }
}

/// Point-in-rounded-rect: inside the rect (edges inclusive) and, within a
/// corner's quadrant, inside that corner's ellipse.
pub(crate) fn rounded_rect_contains(
    rect: Rect<f32>,
    radii: &CornerRadii,
    point: Point2D<f32>,
) -> bool {
    let min = rect.origin;
    let max = Point2D::new(
        rect.origin.x + rect.size.width,
        rect.origin.y + rect.size.height,
    );
    if point.x < min.x || point.x > max.x || point.y < min.y || point.y > max.y {
        return false;
    }
    if radii.is_zero() {
        return true;
    }
    let inside_ellipse = |center: Point2D<f32>, radius: Size2D<f32>| {
        let dx = (point.x - center.x) / radius.width;
        let dy = (point.y - center.y) / radius.height;
        dx * dx + dy * dy <= 1.0
    };
    let corner = |radius: Size2D<f32>| radius.width > 0.0 && radius.height > 0.0;

    if corner(radii.top_left)
        && point.x < min.x + radii.top_left.width
        && point.y < min.y + radii.top_left.height
    {
        return inside_ellipse(
            Point2D::new(min.x + radii.top_left.width, min.y + radii.top_left.height),
            radii.top_left,
        );
    }
    if corner(radii.top_right)
        && point.x > max.x - radii.top_right.width
        && point.y < min.y + radii.top_right.height
    {
        return inside_ellipse(
            Point2D::new(
                max.x - radii.top_right.width,
                min.y + radii.top_right.height,
            ),
            radii.top_right,
        );
    }
    if corner(radii.bottom_right)
        && point.x > max.x - radii.bottom_right.width
        && point.y > max.y - radii.bottom_right.height
    {
        return inside_ellipse(
            Point2D::new(
                max.x - radii.bottom_right.width,
                max.y - radii.bottom_right.height,
            ),
            radii.bottom_right,
        );
    }
    if corner(radii.bottom_left)
        && point.x < min.x + radii.bottom_left.width
        && point.y > max.y - radii.bottom_left.height
    {
        return inside_ellipse(
            Point2D::new(
                min.x + radii.bottom_left.width,
                max.y - radii.bottom_left.height,
            ),
            radii.bottom_left,
        );
    }
    true
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn radii(uniform: f32) -> CornerRadii {
        let corner = Size2D::new(uniform, uniform);
        CornerRadii {
            top_left: corner,
            top_right: corner,
            bottom_right: corner,
            bottom_left: corner,
        }
    }

    #[test]
    fn overlapping_radii_scale_down_uniformly() {
        // Radii of 80 on a 100-wide edge overlap: factor = 100 / 160.
        let mut oversized = radii(80.0);
        let rect = Rect::from_size(Size2D::new(100.0, 100.0));
        // Normalization itself lives in resolve_corner_radii; replicate the
        // factor here against the containment test: an 80px corner on a
        // 100px box would swallow the midpoint of each edge.
        let factor = 100.0 / 160.0;
        for corner in [
            &mut oversized.top_left,
            &mut oversized.top_right,
            &mut oversized.bottom_right,
            &mut oversized.bottom_left,
        ] {
            corner.width *= factor;
            corner.height *= factor;
        }
        // Edge midpoints lie exactly on the rounded outline once normalized.
        assert!(rounded_rect_contains(
            rect,
            &oversized,
            Point2D::new(50.0, 0.0)
        ));
        assert!(rounded_rect_contains(
            rect,
            &oversized,
            Point2D::new(0.0, 50.0)
        ));
    }

    #[test]
    fn corner_quadrants_use_the_ellipse() {
        let rect = Rect::from_size(Size2D::new(100.0, 100.0));
        let rounded = radii(50.0);
        assert!(rounded_rect_contains(
            rect,
            &rounded,
            Point2D::new(50.0, 50.0)
        ));
        // (10, 10) is inside the corner square but outside the circle of
        // radius 50 centered at (50, 50).
        assert!(!rounded_rect_contains(
            rect,
            &rounded,
            Point2D::new(10.0, 10.0)
        ));
        // On-axis edge points stay inside.
        assert!(rounded_rect_contains(
            rect,
            &rounded,
            Point2D::new(50.0, 0.0)
        ));
        // Outside the rect entirely.
        assert!(!rounded_rect_contains(
            rect,
            &rounded,
            Point2D::new(101.0, 50.0)
        ));
    }

    #[test]
    fn inner_radii_inset_by_borders_and_clamp_at_zero() {
        let outer = radii(10.0);
        let border = Edges {
            left: 4.0,
            right: 20.0,
            top: 2.0,
            bottom: 20.0,
        };
        let inner = inner_radii(outer, &border);
        assert_eq!(inner.top_left, Size2D::new(6.0, 8.0));
        assert_eq!(inner.top_right, Size2D::new(0.0, 8.0));
        assert_eq!(inner.bottom_right, Size2D::new(0.0, 0.0));
        assert_eq!(inner.bottom_left, Size2D::new(6.0, 0.0));
    }
}
