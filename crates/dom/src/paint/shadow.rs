//! `box-shadow` (css-backgrounds-3 §7): outset shadows under the box,
//! inset shadows over the background.
//!
//! The computed list paints **last shadow first**, so the first-specified
//! shadow ends up on top (css-backgrounds-3 §7.4.1). CSS blur-radius is 2σ;
//! vello's blur primitive cuts the Gaussian off at 2.5σ.
//!
//! - Outset: [`Scene::draw_blurred_rounded_rect`] of the border-box rect offset by the shadow
//!   offset and inflated by the spread, inside an even-odd clip that carves the border-box shape
//!   out (§7.4.1: the shadow paints only outside the border box). Zero-blur shadows fill the offset
//!   shape exactly, with per-corner radii.
//! - Inset: inside a full `SrcOver` layer clipped to the padding-box shape (a
//!   full layer, not a clip layer, because a blend layer sits inside —
//!   vello [#1198](https://github.com/linebender/vello/issues/1198)), fill
//!   the padding box with the shadow color, then erase the "hole" (padding
//!   box offset by the shadow offset, deflated by the spread) with a
//!   `Compose::DestOut` blurred full-alpha white draw: what survives is
//!   color × (1 − blurred hole alpha) — exactly the inset shadow band.
//!   Zero-blur insets fill the (padding − hole) ring even-odd under a plain
//!   clip layer instead.
//!
//! Recorded approximations:
//! - vello's blur primitive takes **one uniform corner radius**: we use the average of the eight
//!   (spread-adjusted) radius components. Zero radii average to zero, so sharp boxes keep sharp
//!   shadows.
//! - Spread adjusts corner radii linearly, clamped at zero, with zero components staying zero
//!   (sharp corners stay sharp — the endpoint of the spec's easing); the nonlinear easing for radii
//!   smaller than the spread is skipped (behavioral fidelity — see AGENTS.md).
//!
//! `currentcolor` defaults resolve via [`convert::resolve_color`].

use stylo::properties::ComputedValues;
use stylo::values::computed::effects::BoxShadow;

use crate::Size2D;
use crate::paint::shape::{BoxShape, inner_radii, normalize_radii, ring_path_into, with_shape};
use crate::paint::{BoxFragment, PathScratch, convert};
use crate::vello::Scene;
use crate::vello::kurbo::Rect;
use crate::vello::peniko::{BlendMode, Color, Compose, Fill, Mix};
use crate::visual::CornerRadii;

pub(crate) fn paint_outset(
    scene: &mut Scene,
    paths: &mut PathScratch,
    style: &ComputedValues,
    fragment: &BoxFragment,
) {
    let shadows = &style.get_effects().box_shadow.0;
    if shadows.iter().all(|shadow| shadow.inset) {
        return;
    }
    let border_shape = BoxShape::new(fragment.border_box, &fragment.radii);
    for shadow in shadows.iter().rev().filter(|shadow| !shadow.inset) {
        paint_one_outset(scene, paths, style, fragment, &border_shape, shadow);
    }
}

fn paint_one_outset(
    scene: &mut Scene,
    paths: &mut PathScratch,
    style: &ComputedValues,
    fragment: &BoxFragment,
    border_shape: &BoxShape,
    shadow: &BoxShadow,
) {
    let color = convert::resolve_color(style, &shadow.base.color);
    if color.components[3] <= 0.0 {
        return;
    }
    let geometry = ShadowGeometry::new(shadow);
    let Some(rect) = offset_rect(fragment.border_box, &geometry, geometry.spread) else {
        return;
    };
    let radii = adjust_radii(&fragment.radii, geometry.spread as f32);

    let margin = 2.5 * geometry.sigma + geometry.dx.abs() + geometry.dy.abs();
    let bounds = BoxShape::Rect(rect.inflate(margin, margin));
    ring_path_into(&mut paths.ring, &bounds, border_shape);
    scene.push_clip_layer(Fill::EvenOdd, fragment.transform, &paths.ring);
    if geometry.sigma > 0.0 {
        scene.draw_blurred_rounded_rect(
            fragment.transform,
            rect,
            color,
            uniform_radius(&radii, rect),
            geometry.sigma,
        );
    } else {
        let radii = normalize_radii(radii, rect.width() as f32, rect.height() as f32);
        let shape = BoxShape::new(rect, &radii);
        with_shape!(&shape, |s| scene.fill(
            Fill::NonZero,
            fragment.transform,
            color,
            None,
            s
        ));
    }
    scene.pop_layer();
}

pub(crate) fn paint_inset(
    scene: &mut Scene,
    paths: &mut PathScratch,
    style: &ComputedValues,
    fragment: &BoxFragment,
) {
    let shadows = &style.get_effects().box_shadow.0;
    if !shadows.iter().any(|shadow| shadow.inset) {
        return;
    }
    let padding_radii = inner_radii(&fragment.radii, &fragment.border_widths);
    let padding_shape = BoxShape::new(fragment.padding_box, &padding_radii);
    for shadow in shadows.iter().rev().filter(|shadow| shadow.inset) {
        paint_one_inset(
            scene,
            paths,
            style,
            fragment,
            &padding_shape,
            &padding_radii,
            shadow,
        );
    }
}

fn paint_one_inset(
    scene: &mut Scene,
    paths: &mut PathScratch,
    style: &ComputedValues,
    fragment: &BoxFragment,
    padding_shape: &BoxShape,
    padding_radii: &CornerRadii,
    shadow: &BoxShadow,
) {
    let color = convert::resolve_color(style, &shadow.base.color);
    if color.components[3] <= 0.0 {
        return;
    }
    let geometry = ShadowGeometry::new(shadow);
    let hole_rect = offset_rect(fragment.padding_box, &geometry, -geometry.spread);
    let hole_radii = adjust_radii(padding_radii, -geometry.spread as f32);

    if geometry.sigma > 0.0 {
        with_shape!(padding_shape, |s| scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            1.0,
            fragment.transform,
            s,
        ));
        with_shape!(padding_shape, |s| scene.fill(
            Fill::NonZero,
            fragment.transform,
            color,
            None,
            s
        ));
        if let Some(hole) = hole_rect {
            with_shape!(padding_shape, |s| scene.push_layer(
                Fill::NonZero,
                BlendMode::new(Mix::Normal, Compose::DestOut),
                1.0,
                fragment.transform,
                s,
            ));
            scene.draw_blurred_rounded_rect(
                fragment.transform,
                hole,
                Color::WHITE,
                uniform_radius(&hole_radii, hole),
                geometry.sigma,
            );
            scene.pop_layer();
        }
        scene.pop_layer();
    } else {
        with_shape!(padding_shape, |s| scene.push_clip_layer(
            Fill::NonZero,
            fragment.transform,
            s
        ));
        match hole_rect {
            Some(hole) => {
                let radii = normalize_radii(hole_radii, hole.width() as f32, hole.height() as f32);
                let hole_shape = BoxShape::new(hole, &radii);
                ring_path_into(&mut paths.ring, padding_shape, &hole_shape);
                scene.fill(Fill::EvenOdd, fragment.transform, color, None, &paths.ring);
            }
            None => with_shape!(padding_shape, |s| scene.fill(
                Fill::NonZero,
                fragment.transform,
                color,
                None,
                s
            )),
        }
        scene.pop_layer();
    }
}

pub(crate) fn extent(style: &ComputedValues) -> f64 {
    outset_extent(&style.get_effects().box_shadow.0)
}

fn outset_extent(shadows: &[BoxShadow]) -> f64 {
    shadows
        .iter()
        .filter(|shadow| !shadow.inset)
        .map(|shadow| {
            let geometry = ShadowGeometry::new(shadow);
            geometry.dx.abs().max(geometry.dy.abs())
                + geometry.spread.max(0.0)
                + 2.5 * geometry.sigma
        })
        .fold(0.0, f64::max)
}

/// One computed shadow's used scalar geometry, in CSS px.
struct ShadowGeometry {
    dx: f64,
    dy: f64,
    spread: f64,
    sigma: f64,
}

impl ShadowGeometry {
    fn new(shadow: &BoxShadow) -> Self {
        Self {
            dx: f64::from(shadow.base.horizontal.px()),
            dy: f64::from(shadow.base.vertical.px()),
            spread: f64::from(shadow.spread.px()),
            sigma: f64::from(shadow.base.blur.px()) / 2.0,
        }
    }
}

fn offset_rect(rect: Rect, geometry: &ShadowGeometry, outset: f64) -> Option<Rect> {
    let rect = Rect::new(
        rect.x0 + geometry.dx - outset,
        rect.y0 + geometry.dy - outset,
        rect.x1 + geometry.dx + outset,
        rect.y1 + geometry.dy + outset,
    );
    (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
}

fn adjust_radii(radii: &CornerRadii, delta: f32) -> CornerRadii {
    let component = |value: f32| {
        if value > 0.0 {
            (value + delta).max(0.0)
        } else {
            0.0
        }
    };
    let corner =
        |corner: Size2D<f32>| Size2D::new(component(corner.width), component(corner.height));
    CornerRadii {
        top_left: corner(radii.top_left),
        top_right: corner(radii.top_right),
        bottom_right: corner(radii.bottom_right),
        bottom_left: corner(radii.bottom_left),
    }
}

fn uniform_radius(radii: &CornerRadii, rect: Rect) -> f64 {
    let sum = radii.top_left.width
        + radii.top_left.height
        + radii.top_right.width
        + radii.top_right.height
        + radii.bottom_right.width
        + radii.bottom_right.height
        + radii.bottom_left.width
        + radii.bottom_left.height;
    (f64::from(sum) / 8.0).min(0.5 * rect.width().min(rect.height()))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::values::computed::effects::SimpleShadow;
    use stylo::values::computed::{Color as StyloColor, Length, NonNegativeLength};

    use super::*;

    fn shadow(dx: f32, dy: f32, blur: f32, spread: f32, inset: bool) -> BoxShadow {
        BoxShadow {
            base: SimpleShadow {
                color: StyloColor::CurrentColor,
                horizontal: Length::new(dx),
                vertical: Length::new(dy),
                blur: NonNegativeLength::new(blur),
            },
            spread: Length::new(spread),
            inset,
        }
    }

    #[test]
    fn extent_is_zero_without_outset_shadows() {
        assert!((outset_extent(&[]) - 0.0).abs() < 1e-9);
        assert!((outset_extent(&[shadow(50.0, 50.0, 50.0, 50.0, true)]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn extent_takes_the_max_outset_reach() {
        let shadows = [
            shadow(4.0, -6.0, 8.0, 3.0, false),
            shadow(100.0, 100.0, 100.0, 100.0, true),
            shadow(2.0, 2.0, 0.0, 0.0, false),
        ];
        assert!((outset_extent(&shadows) - 19.0).abs() < 1e-9);
    }

    #[test]
    fn extent_ignores_negative_spread() {
        assert!((outset_extent(&[shadow(0.0, 0.0, 4.0, -10.0, false)]) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn offset_rect_translates_and_inflates() {
        let geometry = ShadowGeometry::new(&shadow(5.0, -2.0, 0.0, 0.0, false));
        let rect = offset_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &geometry, 3.0).unwrap();
        assert!((rect.x0 - 2.0).abs() < 1e-9);
        assert!((rect.y0 - -5.0).abs() < 1e-9);
        assert!((rect.x1 - 18.0).abs() < 1e-9);
        assert!((rect.y1 - 11.0).abs() < 1e-9);
    }

    #[test]
    fn offset_rect_collapses_to_none() {
        let geometry = ShadowGeometry::new(&shadow(0.0, 0.0, 0.0, 0.0, false));
        assert!(offset_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &geometry, -5.0).is_none());
        assert!(offset_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &geometry, -6.0).is_none());
        assert!(offset_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &geometry, -4.9).is_some());
    }

    #[test]
    fn adjust_radii_keeps_sharp_corners_sharp() {
        let radii = CornerRadii {
            top_left: Size2D::new(4.0, 6.0),
            top_right: Size2D::new(0.0, 3.0),
            bottom_right: Size2D::new(2.0, 2.0),
            bottom_left: Size2D::new(0.0, 0.0),
        };
        let inflated = adjust_radii(&radii, 2.0);
        assert!((inflated.top_left.width - 6.0).abs() < 1e-6);
        assert!((inflated.top_left.height - 8.0).abs() < 1e-6);
        assert!((inflated.top_right.width - 0.0).abs() < 1e-6);
        assert!((inflated.top_right.height - 5.0).abs() < 1e-6);
        assert!(inflated.bottom_left.width.abs() < 1e-6);
        assert!(inflated.bottom_left.height.abs() < 1e-6);
    }

    #[test]
    fn adjust_radii_clamps_deflation_at_zero() {
        let radii = CornerRadii {
            top_left: Size2D::new(4.0, 6.0),
            top_right: Size2D::new(1.0, 1.0),
            bottom_right: Size2D::new(5.0, 5.0),
            bottom_left: Size2D::new(0.0, 0.0),
        };
        let deflated = adjust_radii(&radii, -5.0);
        assert!(deflated.top_left.width.abs() < 1e-6);
        assert!((deflated.top_left.height - 1.0).abs() < 1e-6);
        assert!(deflated.top_right.width.abs() < 1e-6);
        assert!(deflated.bottom_right.width.abs() < 1e-6);
    }

    #[test]
    fn uniform_radius_averages_all_components() {
        let radii = CornerRadii {
            top_left: Size2D::new(8.0, 4.0),
            top_right: Size2D::new(2.0, 6.0),
            bottom_right: Size2D::new(0.0, 0.0),
            bottom_left: Size2D::new(3.0, 1.0),
        };
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!((uniform_radius(&radii, rect) - 3.0).abs() < 1e-9);
        assert!(uniform_radius(&CornerRadii::ZERO, rect).abs() < 1e-9);
    }

    #[test]
    fn uniform_radius_clamps_to_half_min_dimension() {
        let radii = CornerRadii {
            top_left: Size2D::new(100.0, 100.0),
            top_right: Size2D::new(100.0, 100.0),
            bottom_right: Size2D::new(100.0, 100.0),
            bottom_left: Size2D::new(100.0, 100.0),
        };
        let rect = Rect::new(0.0, 0.0, 10.0, 40.0);
        assert!((uniform_radius(&radii, rect) - 5.0).abs() < 1e-9);
    }
}
