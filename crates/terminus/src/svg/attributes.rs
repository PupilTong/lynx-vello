//! Attribute values of SVG elements read as attributes rather than through
//! the cascade: the root's sizes, `viewBox`, `preserveAspectRatio`, lengths
//! and length lists, opacities, transform lists, and the `href` reference.

use std::str::FromStr;

use svgtypes::{Length, LengthListParser, LengthUnit};

use super::gradient::plain;
use crate::kurbo::Affine;
use crate::render::image::{AspectAlign, AspectRatio};

/// `node`'s `href`, or its `xlink:href` when it has no `href`: SVG 2
/// ("Deprecated `XLink` URL reference attributes") gives `href`
/// precedence. Every reader of an SVG element's reference goes through it.
pub(super) fn href<T>(node: &crate::tree::node::Node<T>) -> Option<&str> {
    plain(node, "href").or_else(|| plain(node, "xlink:href"))
}

/// The default object size of CSS Images 3, in CSS px.
const DEFAULT_OBJECT_SIZE: (f32, f32) = (300.0, 150.0);

/// A root `width` or `height` that is an absolute length, in px: a bare
/// number, or a number in one of the CSS absolute units (`px`; `in` = 96 px,
/// `cm` = 96/2.54 px, `mm` = 96/25.4 px, `pt` = 4/3 px, `pc` = 16 px),
/// finite and positive. Anything else, `em`, `ex` and `%` included, counts
/// as absent.
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

/// A `viewBox` with a positive, finite width and height.
pub(super) fn view_box(value: &str) -> Option<svgtypes::ViewBox> {
    svgtypes::ViewBox::from_str(value)
        .ok()
        .filter(|v| v.w.is_finite() && v.h.is_finite() && v.w > 0.0 && v.h > 0.0)
}

/// The natural size and viewport of a document from its root's absolute
/// `width` and `height` and its `viewBox` size.
///
/// The natural size is CSS Images 3 default sizing: both absolute → that
/// size; one absolute plus a `viewBox` → the other axis from the `viewBox`
/// ratio; a `viewBox` only → the largest size with its ratio that fits
/// 300×150; neither → 300×150; one absolute and no `viewBox` → that axis
/// with 300 or 150 for the other. Rounded to whole px, at least 1.
///
/// The viewport, the rectangle the items are drawn in, is the `viewBox`
/// size when there is one; otherwise the authored size when both axes are
/// authored, else the rounded natural size.
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

/// A `preserveAspectRatio` value; an unparsable one is the default.
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

/// The transform that maps `view_box` onto a `width`×`height` viewport
/// under `aspect` (SVG 2 §8.6).
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

/// The viewport percentages resolve against.
#[derive(Clone, Copy, Debug)]
pub(super) struct Viewport {
    pub(super) width: f64,
    pub(super) height: f64,
}

impl Viewport {
    /// The reference for a length that is neither horizontal nor vertical
    /// (`r`, `stroke-width`): SVG's normalised diagonal.
    pub(super) fn diagonal(self) -> f64 {
        f64::midpoint(self.width * self.width, self.height * self.height).sqrt()
    }
}

/// Which axis a length is measured along, for percentages.
#[derive(Clone, Copy)]
pub(super) enum Axis {
    Horizontal,
    Vertical,
    Diagonal,
}

/// `length` in user units: absolute units at 96 px per inch, `em`/`ex`
/// of `font_size`, a percentage of `viewport` along `axis`.
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

/// Parses `value` as one length and resolves it; `None` for a value that
/// is not a length.
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

/// The first length of a list attribute (`x="10 20 30"` reads `10`), the
/// documented subset for `text` positions.
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

/// A `<number>` or `<percentage>` clamped to the unit interval, as every
/// opacity is.
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

/// A `transform` list folded into one affine, in SVG's order (the first
/// function applies last to a point). An invalid list is the identity.
pub(super) fn transform(value: &str) -> Affine {
    transform_list(value).unwrap_or(Affine::IDENTITY)
}

/// [`transform`], `None` for an invalid list. Also what an SVG element's
/// `transform` attribute is reflected into the CSS `transform` property
/// with ([`crate::svg::presentation`]).
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
