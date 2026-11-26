//! Gradients as brushes: a gradient's resolved specification, and the
//! brush that paints it over a bounding box.

use crate::kurbo::{Affine, Rect};
use crate::peniko::color::DynamicColor;
use crate::peniko::{Brush, Color, ColorStop, Extend, Gradient};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Units {
    ObjectBoundingBox,
    UserSpaceOnUse,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Geometry {
    Linear {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    Radial {
        cx: f64,
        cy: f64,
        r: f64,
        fx: f64,
        fy: f64,
        fr: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GradientSpec {
    pub(crate) geometry: Geometry,
    pub(crate) units: Units,
    pub(crate) transform: Affine,
    pub(crate) extend: Extend,
    pub(crate) stops: Vec<(f32, Color)>,
}

pub(crate) fn fold_opacity(color: Color, opacity: f32) -> Color {
    let rgba = color.to_rgba8();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a unit-interval opacity scaled to 0..=255 and rounded"
    )]
    let alpha = (f32::from(rgba.a) * opacity).round() as u8;
    Color::from_rgba8(rgba.r, rgba.g, rgba.b, alpha)
}

pub(crate) fn brush(
    spec: &GradientSpec,
    bbox: Option<Rect>,
    opacity: f32,
) -> Option<(Brush, Option<Affine>)> {
    let (first, rest) = spec.stops.split_first()?;
    let faded = |color: Color| fold_opacity(color, opacity);
    let transform = match spec.units {
        Units::ObjectBoundingBox => {
            let bbox = bbox?;
            if !(bbox.width() > 0.0 && bbox.height() > 0.0) {
                return None;
            }
            Affine::translate((bbox.x0, bbox.y0))
                * Affine::scale_non_uniform(bbox.width(), bbox.height())
                * spec.transform
        }
        Units::UserSpaceOnUse => spec.transform,
    };
    if rest.is_empty() {
        return Some((Brush::Solid(faded(first.1)), None));
    }
    let gradient = match spec.geometry {
        Geometry::Linear { x1, y1, x2, y2 } => Gradient::new_linear((x1, y1), (x2, y2)),
        Geometry::Radial {
            cx,
            cy,
            r,
            fx,
            fy,
            fr,
        } => {
            if r <= 0.0 {
                let last = spec.stops[spec.stops.len() - 1].1;
                return Some((Brush::Solid(faded(last)), None));
            }
            #[expect(clippy::cast_possible_truncation, reason = "peniko takes f32 radii")]
            let (fr, r) = (fr.max(0.0) as f32, r as f32);
            Gradient::new_two_point_radial((fx, fy), fr, (cx, cy), r)
        }
    };
    let stops: Vec<ColorStop> = spec
        .stops
        .iter()
        .map(|(offset, color)| ColorStop {
            offset: *offset,
            color: DynamicColor::from_alpha_color(faded(*color)),
        })
        .collect();
    Some((
        Brush::Gradient(
            gradient
                .with_extend(spec.extend)
                .with_stops(stops.as_slice()),
        ),
        Some(transform),
    ))
}
