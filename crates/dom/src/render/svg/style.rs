//! Properties: where an element's declared values come from, and the
//! computed style each element inherits from its parent.
//!
//! A value has three sources, in rising precedence: the presentation
//! attribute of the same name, the `<style>` rules that match the element
//! (`simplecss`, by specificity then source order), and the `style`
//! attribute; an `!important` rule beats the attribute. [`declarations`]
//! folds them into one small table per element and [`Style::resolve`] turns
//! that table plus the parent's computed style into the element's own.

use std::str::FromStr;

use smallvec::SmallVec;
use svgtypes::{Length, LengthListParser, LengthUnit, PaintFallback};

use crate::vello::kurbo::{Affine, Cap, Join};
use crate::vello::peniko::{Fill, Mix};

/// The property names read as presentation attributes. An attribute outside
/// this list is never a property, so `width` on a `rect` stays geometry.
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
    "isolation",
    "mask",
    "mix-blend-mode",
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

/// One element's declared property values, highest precedence already
/// applied.
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

    fn set(&mut self, name: &'a str, value: &'a str) {
        let value = value.trim();
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|(declared, _)| *declared == name)
        {
            entry.1 = value;
        } else {
            self.entries.push((name, value));
        }
    }
}

/// The document's `<style>` rules, sorted so that applying them in order
/// lets the right one win: ascending specificity, source order within a
/// specificity.
pub(super) struct Sheet<'a> {
    /// Each rule with the fast reject its selector allows.
    rules: Vec<(simplecss::Rule<'a>, RuleFilter)>,
}

impl<'a> Sheet<'a> {
    /// Parses every `<style>` element of `document` (a `type` other than
    /// `text/css` is skipped) into one sheet.
    pub(super) fn collect(document: &'a roxmltree::Document<'a>) -> Self {
        let mut sheet = simplecss::StyleSheet::new();
        for node in document
            .descendants()
            .filter(|node| node.has_tag_name("style"))
        {
            if node
                .attribute("type")
                .is_some_and(|kind| !kind.trim().eq_ignore_ascii_case("text/css"))
            {
                continue;
            }
            for text in node.children().filter_map(|child| child.text()) {
                sheet.parse_more(text);
            }
        }
        let mut rules = sheet.rules;
        rules.sort_by_key(|rule| rule.selector.specificity());
        Self {
            rules: rules
                .into_iter()
                .map(|rule| {
                    let filter = rule_filter(&rule.selector);
                    (rule, filter)
                })
                .collect(),
        }
    }

    fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// What an element must have for a rule to match at all: the type, class
/// and id its selector's rightmost compound names. Most rules of a sheet
/// (an Illustrator export's `.st0 … .st40`) are rejected by one string
/// compare instead of a selector walk per element.
#[derive(Default)]
struct RuleFilter {
    tag: Option<String>,
    class: Option<String>,
    id: Option<String>,
}

impl RuleFilter {
    /// Whether an element with these attributes may match.
    fn admits(&self, tag: &str, class: Option<&str>, id: Option<&str>) -> bool {
        self.tag.as_deref().is_none_or(|required| required == tag)
            && self.class.as_deref().is_none_or(|required| {
                class.is_some_and(|class| {
                    class.split_ascii_whitespace().any(|word| word == required)
                })
            })
            && self
                .id
                .as_deref()
                .is_none_or(|required| id == Some(required))
    }
}

/// The filter for `selector`, read back from the text `simplecss` prints
/// it as (`g > *[class~='c']`): the rightmost compound's type, `[class~=]`
/// and `[id=]`. Anything the reader does not recognise adds no
/// requirement, so a filter only ever rejects what the selector would.
fn rule_filter(selector: &simplecss::Selector<'_>) -> RuleFilter {
    let text = selector.to_string();
    // The rightmost compound starts after the last combinator outside
    // brackets.
    let mut depth = 0_u32;
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => depth = depth.saturating_sub(1),
            b' ' | b'>' | b'+' if depth == 0 => start = index + 1,
            _ => {}
        }
    }
    let compound = &text[start..];
    let mut filter = RuleFilter::default();
    let tag_end = compound.find(['[', ':']).unwrap_or(compound.len());
    let tag = &compound[..tag_end];
    if !tag.is_empty() && tag != "*" {
        filter.tag = Some(tag.to_owned());
    }
    let mut rest = &compound[tag_end..];
    while let Some(inner) = rest.strip_prefix('[') {
        let Some(end) = inner.find(']') else { break };
        let attribute = &inner[..end];
        let quoted = |prefix: &str| {
            attribute
                .strip_prefix(prefix)
                .and_then(|value| value.strip_suffix('\''))
                .filter(|value| !value.contains('\''))
                .map(str::to_owned)
        };
        if let Some(class) = quoted("class~='") {
            filter.class = Some(class);
        } else if let Some(id) = quoted("id='") {
            filter.id = Some(id);
        }
        rest = &inner[end + 1..];
    }
    filter
}

/// A roxmltree element as `simplecss` matches selectors against it.
struct XmlElement<'a, 'input>(roxmltree::Node<'a, 'input>);

impl simplecss::Element for XmlElement<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(XmlElement)
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(XmlElement)
    }

    fn has_local_name(&self, name: &str) -> bool {
        self.0.tag_name().name() == name
    }

    fn attribute_matches(
        &self,
        local_name: &str,
        operator: simplecss::AttributeOperator<'_>,
    ) -> bool {
        self.0
            .attribute(local_name)
            .is_some_and(|value| operator.matches(value))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass<'_>) -> bool {
        match class {
            simplecss::PseudoClass::FirstChild => self.0.prev_sibling_element().is_none(),
            simplecss::PseudoClass::Link
            | simplecss::PseudoClass::Visited
            | simplecss::PseudoClass::Hover
            | simplecss::PseudoClass::Active
            | simplecss::PseudoClass::Focus
            | simplecss::PseudoClass::Lang(_) => false,
        }
    }
}

/// `node`'s declared values from its presentation attributes, the matching
/// rules of `sheet` and its `style` attribute.
pub(super) fn declarations<'a>(
    node: roxmltree::Node<'a, 'a>,
    sheet: &Sheet<'a>,
) -> Declarations<'a> {
    let mut declared = Declarations::default();
    for attribute in node.attributes() {
        if attribute.namespace().is_none() && PROPERTIES.contains(&attribute.name()) {
            declared.set(attribute.name(), attribute.value());
        }
    }
    let mut important: SmallVec<[(&str, &str); 2]> = SmallVec::new();
    if !sheet.is_empty() {
        let element = XmlElement(node);
        let tag = node.tag_name().name();
        let class = node.attribute("class");
        let id = node.attribute("id");
        for (rule, filter) in &sheet.rules {
            if !filter.admits(tag, class, id) || !rule.selector.matches(&element) {
                continue;
            }
            for declaration in &rule.declarations {
                if !PROPERTIES.contains(&declaration.name) {
                    continue;
                }
                if declaration.important {
                    important.push((declaration.name, declaration.value));
                } else {
                    declared.set(declaration.name, declaration.value);
                }
            }
        }
    }
    if let Some(style) = node.attribute("style") {
        for declaration in simplecss::DeclarationTokenizer::from(style) {
            if PROPERTIES.contains(&declaration.name) {
                declared.set(declaration.name, declaration.value);
            }
        }
    }
    for (name, value) in important {
        declared.set(name, value);
    }
    declared
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
    pub(super) blend: Mix,
    pub(super) isolate: bool,
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
            blend: Mix::Normal,
            isolate: false,
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
            blend: initial.blend,
            isolate: initial.isolate,
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
                "mix-blend-mode" => {
                    if let Some(mix) = blend_mode(value) {
                        style.blend = mix;
                    }
                }
                "isolation" => style.isolate = *value == "isolate",
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
        if inherit("mix-blend-mode") {
            style.blend = parent.blend;
        }
        if inherit("isolation") {
            style.isolate = parent.isolate;
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

fn blend_mode(value: &str) -> Option<Mix> {
    Some(match value {
        "normal" => Mix::Normal,
        "multiply" => Mix::Multiply,
        "screen" => Mix::Screen,
        "overlay" => Mix::Overlay,
        "darken" => Mix::Darken,
        "lighten" => Mix::Lighten,
        "color-dodge" => Mix::ColorDodge,
        "color-burn" => Mix::ColorBurn,
        "hard-light" => Mix::HardLight,
        "soft-light" => Mix::SoftLight,
        "difference" => Mix::Difference,
        "exclusion" => Mix::Exclusion,
        "hue" => Mix::Hue,
        "saturation" => Mix::Saturation,
        "color" => Mix::Color,
        "luminosity" => Mix::Luminosity,
        _ => return None,
    })
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
