//! Attribute values of SVG elements read as attributes rather than through
//! the cascade: the root's sizes, `viewBox` and transform lists.

use std::str::FromStr;

use crate::kurbo::Affine;

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

/// A `transform` list as one affine, `None` for an invalid list. Also what an SVG element's
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
