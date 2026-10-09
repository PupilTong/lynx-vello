//! Properties: where an element's declared values come from, and the
//! computed style each element inherits from its parent.
//!
//! A value has one source, the presentation attribute of the same name: CSS
//! inside an SVG document (`<style>` rules, the `style` attribute) is not
//! read. [`declarations`] collects an element's attributes into one small
//! table and [`Style::resolve`] turns that table plus the parent's computed
//! style into the element's own.

use std::str::FromStr;

use smallvec::SmallVec;
use svgtypes::{Length, LengthListParser, LengthUnit, PaintFallback};

use crate::vello::kurbo::{Affine, Cap, Join};
use crate::vello::peniko::Fill;

/// The property names read as presentation attributes. An attribute outside
/// this list is never a property, so `width` on a `rect` stays geometry.
/// `mix-blend-mode` and `isolation` are not here: SVG 2 gives them no
/// presentation attribute, only CSS sets them, and CSS is not read, so a
/// document cannot set them at all.
const PROPERTIES: &[&str] = &[
    "clip-path",
    "clip-rule",
    "color",
    "display",
    "fill",
    "fill-opacity",
    "fill-rule",
    "filter",
    "font-size",
    "mask",
    "opacity",
    "paint-order",
    "stop-color",
    "stop-opacity",
    "stroke",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-opacity",
    "stroke-width",
    "transform",
    "visibility",
];

/// One element's declared property values: its presentation attributes
/// named in [`PROPERTIES`], values trimmed.
#[derive(Default)]
pub(super) struct Declarations<'a> {
    entries: SmallVec<[(&'a str, &'a str); 8]>,
}

impl<'a> Declarations<'a> {
    pub(super) fn get(&self, name: &str) -> Option<&'a str> {
        self.entries
            .iter()
            .find(|(declared, _)| *declared == name)
            .map(|(_, value)| *value)
    }
}

/// `node`'s declared values: its presentation attributes, and nothing else.
/// Neither a `style` attribute nor a `<style>` element is read (the
/// "CSS inside SVG" ruling of `docs/svg-lynx-component-design.md`). XML
/// allows an attribute once per element, so each property has at most one
/// value.
pub(super) fn declarations<'a>(node: roxmltree::Node<'a, 'a>) -> Declarations<'a> {
    Declarations {
        entries: node
            .attributes()
            .filter(|attribute| {
                attribute.namespace().is_none() && PROPERTIES.contains(&attribute.name())
            })
            .map(|attribute| (attribute.name(), attribute.value().trim()))
            .collect(),
    }
}

/// A paint as declared: resolved to a brush where it is used, against the
/// shape it fills.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Paint {
    None,
    Color(svgtypes::Color),
    /// Resolved against the using element's own `color`.
    CurrentColor,
    /// `url(#id)`, with the fallback used when the reference does not
    /// resolve.
    Server {
        id: String,
        fallback: Option<Fallback>,
    },
}

/// What a `url()` paint falls back to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Fallback {
    None,
    CurrentColor,
    Color(svgtypes::Color),
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

/// The computed style of one element.
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one two-keyword SVG property, read by name where it applies"
)]
pub(super) struct Style {
    // Inherited.
    pub(super) fill: Paint,
    pub(super) fill_opacity: f32,
    pub(super) fill_rule: Fill,
    pub(super) stroke: Paint,
    pub(super) stroke_width: f64,
    pub(super) stroke_opacity: f32,
    pub(super) line_cap: Cap,
    pub(super) line_join: Join,
    pub(super) miter_limit: f64,
    pub(super) dash_array: Option<Vec<f64>>,
    pub(super) dash_offset: f64,
    /// `paint-order` names the stroke before the fill.
    pub(super) stroke_first: bool,
    /// The `color` property, what `currentColor` resolves to.
    pub(super) color: svgtypes::Color,
    pub(super) visible: bool,
    /// `font-size` in px, what `em` and `ex` lengths resolve against.
    pub(super) font_size: f32,
    pub(super) clip_rule: Fill,
    // Not inherited.
    /// `display` is not `none`.
    pub(super) display: bool,
    pub(super) opacity: f32,
    pub(super) clip_path: Option<String>,
    pub(super) masked: bool,
    /// The element's own `transform`.
    pub(super) transform: Affine,
}

impl Style {
    /// The specification's initial values, the style of the document root's
    /// parent.
    pub(super) fn initial() -> Self {
        Self {
            fill: Paint::Color(svgtypes::Color::black()),
            fill_opacity: 1.0,
            fill_rule: Fill::NonZero,
            stroke: Paint::None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            line_cap: Cap::Butt,
            line_join: Join::Miter,
            miter_limit: 4.0,
            dash_array: None,
            dash_offset: 0.0,
            stroke_first: false,
            color: svgtypes::Color::black(),
            visible: true,
            font_size: 16.0,
            clip_rule: Fill::NonZero,
            display: true,
            opacity: 1.0,
            clip_path: None,
            masked: false,
            transform: Affine::IDENTITY,
        }
    }

    /// The computed style of an element with `declared` values under a
    /// parent of style `self`, in `viewport`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per property; splitting it would scatter the inheritance rule"
    )]
    pub(super) fn resolve(&self, declared: &Declarations<'_>, viewport: Viewport) -> Self {
        let parent = self;
        // Inherited properties start as the parent's; the rest as initial.
        let initial = Self::initial();
        let mut style = Self {
            display: initial.display,
            opacity: initial.opacity,
            clip_path: None,
            masked: false,
            transform: Affine::IDENTITY,
            ..parent.clone()
        };
        // `inherit` on a non-inherited property takes the parent's value.
        let inherit = |name: &str| declared.get(name) == Some("inherit");
        // Font size first: `em` lengths below resolve against it.
        if let Some(value) = declared
            .get("font-size")
            .filter(|value| *value != "inherit")
            && let Some(size) = font_size(value, f64::from(parent.font_size), viewport)
        {
            #[expect(clippy::cast_possible_truncation, reason = "a font size fits an f32")]
            let size = size as f32;
            style.font_size = size;
        }
        let font_size = f64::from(style.font_size);
        let length = |value: &str, axis: Axis| parse_length(value, font_size, viewport, axis);

        for (name, value) in declared
            .entries
            .iter()
            .filter(|(_, value)| *value != "inherit")
        {
            match *name {
                "fill" => {
                    if let Some(paint) = paint(value) {
                        style.fill = paint;
                    }
                }
                "fill-opacity" => {
                    if let Some(opacity) = parse_opacity(value) {
                        style.fill_opacity = opacity;
                    }
                }
                "fill-rule" => {
                    if let Some(rule) = fill_rule(value) {
                        style.fill_rule = rule;
                    }
                }
                "clip-rule" => {
                    if let Some(rule) = fill_rule(value) {
                        style.clip_rule = rule;
                    }
                }
                "stroke" => {
                    if let Some(paint) = paint(value) {
                        style.stroke = paint;
                    }
                }
                "stroke-width" => {
                    if let Some(width) = length(value, Axis::Diagonal).filter(|width| *width >= 0.0)
                    {
                        style.stroke_width = width;
                    }
                }
                "stroke-opacity" => {
                    if let Some(opacity) = parse_opacity(value) {
                        style.stroke_opacity = opacity;
                    }
                }
                "stroke-linecap" => {
                    style.line_cap = match *value {
                        "butt" => Cap::Butt,
                        "round" => Cap::Round,
                        "square" => Cap::Square,
                        _ => style.line_cap,
                    };
                }
                "stroke-linejoin" => {
                    style.line_join = match *value {
                        // kurbo has no clipped miter; a plain miter is the
                        // nearest join.
                        "miter" | "miter-clip" => Join::Miter,
                        "round" => Join::Round,
                        "bevel" => Join::Bevel,
                        _ => style.line_join,
                    };
                }
                "stroke-miterlimit" => {
                    if let Ok(limit) = value.parse::<f64>()
                        && limit >= 1.0
                    {
                        style.miter_limit = limit;
                    }
                }
                "stroke-dasharray" => {
                    if *value == "none" {
                        style.dash_array = None;
                    } else if let Some(dashes) = dash_array(value, font_size, viewport) {
                        style.dash_array = (!dashes.is_empty()).then_some(dashes);
                    }
                }
                "stroke-dashoffset" => {
                    if let Some(offset) = length(value, Axis::Diagonal) {
                        style.dash_offset = offset;
                    }
                }
                "paint-order" => {
                    if let Ok(order) = svgtypes::PaintOrder::from_str(value) {
                        style.stroke_first = order.order[0] == svgtypes::PaintOrderKind::Stroke
                            || (order.order[0] == svgtypes::PaintOrderKind::Markers
                                && order.order[1] == svgtypes::PaintOrderKind::Stroke);
                    }
                }
                "color" => {
                    if let Ok(color) = svgtypes::Color::from_str(value) {
                        style.color = color;
                    }
                }
                "visibility" => {
                    style.visible = match *value {
                        "visible" => true,
                        "hidden" | "collapse" => false,
                        _ => style.visible,
                    };
                }
                "display" => style.display = *value != "none",
                "opacity" => {
                    if let Some(opacity) = parse_opacity(value) {
                        style.opacity = opacity;
                    }
                }
                "clip-path" => {
                    style.clip_path = svgtypes::FuncIRI::from_str(value)
                        .ok()
                        .map(|iri| iri.0.to_owned());
                }
                "mask" => style.masked = *value != "none",
                "transform" => style.transform = transform(value),
                // Read elsewhere (`stop`), or ignored (`filter`).
                _ => {}
            }
        }
        if inherit("display") {
            style.display = parent.display;
        }
        if inherit("opacity") {
            style.opacity = parent.opacity;
        }
        if inherit("clip-path") {
            style.clip_path.clone_from(&parent.clip_path);
        }
        if inherit("mask") {
            style.masked = parent.masked;
        }
        if inherit("transform") {
            style.transform = parent.transform;
        }
        style
    }
}

/// A `<paint>`: `none`, a colour, `currentColor`, or `url(#id)` with its
/// fallback. `None` for a value that is not a paint (or is `inherit`, which
/// the caller has already excluded).
fn paint(value: &str) -> Option<Paint> {
    match svgtypes::Paint::from_str(value).ok()? {
        svgtypes::Paint::Inherit => None,
        svgtypes::Paint::CurrentColor => Some(Paint::CurrentColor),
        svgtypes::Paint::Color(color) => Some(Paint::Color(color)),
        svgtypes::Paint::FuncIRI(id, fallback) => Some(Paint::Server {
            id: id.to_owned(),
            fallback: fallback.map(|fallback| match fallback {
                PaintFallback::None => Fallback::None,
                PaintFallback::CurrentColor => Fallback::CurrentColor,
                PaintFallback::Color(color) => Fallback::Color(color),
            }),
        }),
        // `context-fill`/`context-stroke` apply inside markers, which are
        // not drawn, so they paint nothing like `none`.
        svgtypes::Paint::None | svgtypes::Paint::ContextFill | svgtypes::Paint::ContextStroke => {
            Some(Paint::None)
        }
    }
}

fn fill_rule(value: &str) -> Option<Fill> {
    match value {
        "nonzero" => Some(Fill::NonZero),
        "evenodd" => Some(Fill::EvenOdd),
        _ => None,
    }
}

/// `stroke-dasharray` as kurbo's dash pattern: lengths in user units, an
/// odd list repeated to an even one as SVG asks, and a list that is all
/// zero or has a negative entry as no dashes (an empty pattern). `None` for
/// a value that does not parse.
fn dash_array(value: &str, font_size: f64, viewport: Viewport) -> Option<Vec<f64>> {
    let mut dashes = Vec::new();
    for length in LengthListParser::from(value) {
        let length = resolve_length(length.ok()?, font_size, viewport, Axis::Diagonal);
        if length < 0.0 || !length.is_finite() {
            return Some(Vec::new());
        }
        dashes.push(length);
    }
    if dashes.iter().all(|dash| *dash == 0.0) {
        return Some(Vec::new());
    }
    if dashes.len() % 2 == 1 {
        let copy = dashes.clone();
        dashes.extend(copy);
    }
    Some(dashes)
}

/// A `transform` list folded into one affine, in SVG's order (the first
/// function applies last to a point). An invalid list is the identity.
pub(super) fn transform(value: &str) -> Affine {
    let mut affine = Affine::IDENTITY;
    for token in svgtypes::TransformListParser::from(value) {
        let Ok(token) = token else {
            return Affine::IDENTITY;
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
    affine
}

/// A `font-size` value in px: a length (`em`, `ex` and `%` of the parent's
/// size), an absolute keyword, or `larger`/`smaller` relative to the
/// parent.
fn font_size(value: &str, parent: f64, viewport: Viewport) -> Option<f64> {
    let keyword = match value {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        "larger" => Some(parent * 1.2),
        "smaller" => Some(parent / 1.2),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    let length = Length::from_str(value).ok()?;
    let size = match length.unit {
        LengthUnit::Percent => length.number / 100.0 * parent,
        _ => resolve_length(length, parent, viewport, Axis::Diagonal),
    };
    (size.is_finite() && size >= 0.0).then_some(size)
}
