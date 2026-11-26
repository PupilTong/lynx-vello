//! Attribute values of SVG elements read as attributes rather than through
//! the cascade: the root's sizes, `viewBox`, `preserveAspectRatio`, lengths
//! and length lists, opacities, transform lists, and the `href` reference.

use std::str::FromStr;

use svgtypes::{Length, LengthListParser, LengthUnit};

use super::gradient::plain;
use crate::kurbo::Affine;
use crate::render::image::{AspectAlign, AspectRatio};

pub(super) fn href<T>(node: &crate::tree::node::Node<T>) -> Option<&str> {
    plain(node, "href").or_else(|| plain(node, "xlink:href"))
}

const DEFAULT_OBJECT_SIZE: (f32, f32) = (300.0, 150.0);

pub(super) fn absolute_length(value: &str) -> Option<f32> {
    const UNITS: [(&str, f32); 6] = [
        ("px", 1.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("pt", 4.0 / 3.0),
        ("pc", 16.0),
    ];
    let value = value.trim();
    let (number, scale) = UNITS
        .iter()
        .find_map(|&(unit, scale)| value.strip_suffix(unit).map(|number| (number, scale)))
        .unwrap_or((value, 1.0));
    number
        .parse::<f32>()
        .ok()
        .map(|length| length * scale)
        .filter(|length| length.is_finite() && *length > 0.0)
}

pub(super) fn view_box(value: &str) -> Option<svgtypes::ViewBox> {
    svgtypes::ViewBox::from_str(value)
        .ok()
        .filter(|v| v.w.is_finite() && v.h.is_finite() && v.w > 0.0 && v.h > 0.0)
}

pub(super) fn vector_sizes(
    width: Option<f32>,
    height: Option<f32>,
    view_box: Option<(f64, f64)>,
) -> ((u32, u32), (f32, f32)) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a viewBox size fits an f32"
    )]
    let view_box = view_box.map(|(w, h)| (w as f32, h as f32));
    let natural = match (width, height, view_box) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some((box_width, box_height))) => {
            (width, width * box_height / box_width)
        }
        (None, Some(height), Some((box_width, box_height))) => {
            (height * box_width / box_height, height)
        }
        (None, None, Some((box_width, box_height))) => {
            let scale = (DEFAULT_OBJECT_SIZE.0 / box_width).min(DEFAULT_OBJECT_SIZE.1 / box_height);
            (box_width * scale, box_height * scale)
        }
        (width, height, None) => (
            width.unwrap_or(DEFAULT_OBJECT_SIZE.0),
            height.unwrap_or(DEFAULT_OBJECT_SIZE.1),
        ),
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded and clamped to at least one before the cast, which saturates"
    )]
    let whole = |length: f32| length.round().max(1.0) as u32;
    let rounded = (whole(natural.0), whole(natural.1));
    let viewport = match (view_box, width, height) {
        (Some(view_box), _, _) => view_box,
        (None, Some(width), Some(height)) => (width, height),
        (None, _, _) => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a natural size with no viewBox is an authored px length or the default"
            )]
            let viewport = (rounded.0 as f32, rounded.1 as f32);
            viewport
        }
    };
    (rounded, viewport)
}

pub(super) fn aspect_ratio(value: &str) -> AspectRatio {
    use svgtypes::Align;
    let Ok(parsed) = svgtypes::AspectRatio::from_str(value) else {
        return AspectRatio::default();
    };
    let align = match parsed.align {
        Align::None => None,
        Align::XMinYMin => Some((AspectAlign::Min, AspectAlign::Min)),
        Align::XMidYMin => Some((AspectAlign::Mid, AspectAlign::Min)),
        Align::XMaxYMin => Some((AspectAlign::Max, AspectAlign::Min)),
        Align::XMinYMid => Some((AspectAlign::Min, AspectAlign::Mid)),
        Align::XMidYMid => Some((AspectAlign::Mid, AspectAlign::Mid)),
        Align::XMaxYMid => Some((AspectAlign::Max, AspectAlign::Mid)),
        Align::XMinYMax => Some((AspectAlign::Min, AspectAlign::Max)),
        Align::XMidYMax => Some((AspectAlign::Mid, AspectAlign::Max)),
        Align::XMaxYMax => Some((AspectAlign::Max, AspectAlign::Max)),
    };
    AspectRatio {
        align,
        slice: parsed.slice,
    }
}

pub(super) fn view_box_transform(
    view_box: svgtypes::ViewBox,
    aspect: AspectRatio,
    width: f64,
    height: f64,
) -> Affine {
    let sx = width / view_box.w;
    let sy = height / view_box.h;
    let origin = Affine::translate((-view_box.x, -view_box.y));
    let Some((align_x, align_y)) = aspect.align else {
        return Affine::scale_non_uniform(sx, sy) * origin;
    };
    let scale = if aspect.slice { sx.max(sy) } else { sx.min(sy) };
    let factor = |align: AspectAlign| match align {
        AspectAlign::Min => 0.0,
        AspectAlign::Mid => 0.5,
        AspectAlign::Max => 1.0,
    };
    let tx = (width - view_box.w * scale) * factor(align_x);
    let ty = (height - view_box.h * scale) * factor(align_y);
    Affine::translate((tx, ty)) * Affine::scale(scale) * origin
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Viewport {
    pub(super) width: f64,
    pub(super) height: f64,
}

impl Viewport {
    pub(super) fn diagonal(self) -> f64 {
        f64::midpoint(self.width * self.width, self.height * self.height).sqrt()
    }
}

#[derive(Clone, Copy)]
pub(super) enum Axis {
    Horizontal,
    Vertical,
    Diagonal,
}

pub(super) fn resolve_length(
    length: Length,
    font_size: f64,
    viewport: Viewport,
    axis: Axis,
) -> f64 {
    let number = length.number;
    match length.unit {
        LengthUnit::None | LengthUnit::Px => number,
        LengthUnit::Em => number * font_size,
        LengthUnit::Ex => number * font_size / 2.0,
        LengthUnit::In => number * 96.0,
        LengthUnit::Cm => number * 96.0 / 2.54,
        LengthUnit::Mm => number * 96.0 / 25.4,
        LengthUnit::Pt => number * 4.0 / 3.0,
        LengthUnit::Pc => number * 16.0,
        LengthUnit::Percent => {
            number / 100.0
                * match axis {
                    Axis::Horizontal => viewport.width,
                    Axis::Vertical => viewport.height,
                    Axis::Diagonal => viewport.diagonal(),
                }
        }
    }
}

pub(super) fn parse_length(
    value: &str,
    font_size: f64,
    viewport: Viewport,
    axis: Axis,
) -> Option<f64> {
    Length::from_str(value.trim())
        .ok()
        .map(|length| resolve_length(length, font_size, viewport, axis))
        .filter(|length| length.is_finite())
}

pub(super) fn parse_first_length(
    value: &str,
    font_size: f64,
    viewport: Viewport,
    axis: Axis,
) -> Option<f64> {
    LengthListParser::from(value)
        .next()
        .and_then(Result::ok)
        .map(|length| resolve_length(length, font_size, viewport, axis))
        .filter(|length| length.is_finite())
}

pub(super) fn parse_opacity(value: &str) -> Option<f32> {
    let length = Length::from_str(value.trim()).ok()?;
    let number = match length.unit {
        LengthUnit::None => length.number,
        LengthUnit::Percent => length.number / 100.0,
        _ => return None,
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an opacity has no precision to lose"
    )]
    let opacity = number.clamp(0.0, 1.0) as f32;
    Some(opacity)
}

pub(super) fn transform(value: &str) -> Affine {
    transform_list(value).unwrap_or(Affine::IDENTITY)
}

pub(super) fn transform_list(value: &str) -> Option<Affine> {
    let mut affine = Affine::IDENTITY;
    for token in svgtypes::TransformListParser::from(value) {
        let Ok(token) = token else {
            return None;
        };
        let step = match token {
            svgtypes::TransformListToken::Matrix { a, b, c, d, e, f } => {
                Affine::new([a, b, c, d, e, f])
            }
            svgtypes::TransformListToken::Translate { tx, ty } => Affine::translate((tx, ty)),
            svgtypes::TransformListToken::Scale { sx, sy } => Affine::scale_non_uniform(sx, sy),
            svgtypes::TransformListToken::Rotate { angle } => Affine::rotate(angle.to_radians()),
            svgtypes::TransformListToken::SkewX { angle } => {
                Affine::skew(angle.to_radians().tan(), 0.0)
            }
            svgtypes::TransformListToken::SkewY { angle } => {
                Affine::skew(0.0, angle.to_radians().tan())
            }
        };
        affine *= step;
    }
    Some(affine)
}
