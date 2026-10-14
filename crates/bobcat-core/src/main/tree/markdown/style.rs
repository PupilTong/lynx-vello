//! `<markdown>`'s `markdown-style` attribute: a JSON object whose keys name
//! parts of the rendered document and whose values are camel-case CSS
//! declarations, turned into the text of one stylesheet for the component's
//! shadow tree.
//!
//! Ported from web-core's
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XMarkdown/XMarkdownAttributes.ts`:
//! `parseMarkdownStyle` (`:454-466`), `#applyMarkdownStyle` (`:1123-1130`),
//! `buildMarkdownStyleCss` (`:474-505`), `selectorMap` (`:395-421`),
//! `toCssValue` (`:434-445`), `normalizeColor` (`:423-429`), `camelToKebab`
//! (`:431-432`) and `unitlessCssProperties` (`:385-393`). The output is
//! web-core's byte for byte, with one exception: the `image` key's selector
//! (see [`selector_map`]).
//!
//! The attribute is read in its own key order. CSS gives the later of two
//! equally specific declarations the win, so `{ marginTop: 4, margin: 0 }`
//! and `{ margin: 0, marginTop: 4 }` style differently, and web-core walks
//! `Object.entries`, which is insertion order with array-index keys first.
//! `serde_json::Value` keeps a sorted map in this build (no `preserve_order`
//! feature), so the attribute is read into [`Json`], which keeps the order.

use std::borrow::Cow;
use std::fmt;

use rustc_hash::FxHashMap;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// What `markdown-style` sets: the stylesheet text and the two keys web-core
/// reads as configuration rather than CSS (`#applyMarkdownStyle`,
/// `XMarkdownAttributes.ts:1123-1130`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MarkdownStyle {
    /// One rule per selector, joined by `\n`; empty when the attribute is
    /// absent, is not JSON, or is not a JSON object.
    pub(super) css: String,
    /// The `truncation` key: the marker web-core appends when `text-maxline`
    /// clips the content (`#ensureTruncationMarker`, `:1306-1331`).
    pub(super) truncation: Option<Truncation>,
    /// The `typewriterCursor` key: the typewriter animation's cursor
    /// (`#appendTypewriterCursor`, `:1340-1407`).
    pub(super) typewriter_cursor: Option<TypewriterCursor>,
}

/// `markdown-style`'s `truncation` object. web-core reads both fields by
/// truthiness (`:1315-1330`): a falsy `content` is `None` here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Truncation {
    /// The marker text (`truncationType: "text"` or absent), or the id of a
    /// light-DOM child moved into the marker (`truncationType: "view"`).
    /// web-core takes `String(content)`, so a number or a boolean is its
    /// JavaScript string; an object or array value is dropped.
    pub(super) content: Option<String>,
    /// `"text"` or `"view"`; any other value is `None`, which web-core treats
    /// as `"text"` (it only compares against `"view"`).
    pub(super) truncation_type: Option<TruncationType>,
}

/// `truncation.truncationType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TruncationType {
    Text,
    View,
}

/// `markdown-style`'s `typewriterCursor` object (`:1349-1372`). Only string
/// values are kept, and an empty string is `None`: web-core tests both
/// fields by truthiness.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TypewriterCursor {
    /// `"none"` hides the cursor; another value is the id of a child moved
    /// into it.
    pub(super) custom_cursor: Option<String>,
    /// The cursor's `vertical-align`.
    pub(super) vertical_align: Option<String>,
}

/// Reads the `markdown-style` attribute text (`parseMarkdownStyle`,
/// `XMarkdownAttributes.ts:454-466`, then `#applyMarkdownStyle`,
/// `:1123-1130`).
///
/// Text that is not JSON, or is JSON but not an object, is the empty style.
/// web-core also accepts a JSON array there, whose index keys then name no
/// selector and no configuration, so it is the empty style too.
pub(super) fn parse_markdown_style(value: &str) -> MarkdownStyle {
    let Ok(Json::Object(entries)) = serde_json::from_str::<Json>(value) else {
        return MarkdownStyle::default();
    };
    let mut style = MarkdownStyle::default();
    let mut css_entries = Vec::with_capacity(entries.len());
    for (key, value) in js_entries(&entries) {
        match key {
            "truncation" => style.truncation = truncation(value),
            "typewriterCursor" => style.typewriter_cursor = typewriter_cursor(value),
            _ => css_entries.push((key, value)),
        }
    }
    style.css = build_markdown_style_css(&css_entries);
    style
}

/// `buildMarkdownStyleCss` (`XMarkdownAttributes.ts:474-505`): one rule per
/// selector a key maps to, declarations in the value's key order.
fn build_markdown_style_css(style: &[(&str, &Json)]) -> String {
    let mut rules: Vec<String> = Vec::new();
    for &(key, properties) in style {
        // `!properties || typeof properties !== 'object'`: an array is an
        // object too, and `Object.entries` of one is its index entries.
        let declarations: Vec<(Cow<'_, str>, &Json)> = match properties {
            Json::Object(entries) => js_entries(entries)
                .into_iter()
                .map(|(key, value)| (Cow::Borrowed(key), value))
                .collect(),
            Json::Array(items) => array_entries(items),
            _ => continue,
        };
        let custom: String;
        let custom_list: [&str; 1];
        let selectors: &[&str] = if key.starts_with('.') || key.starts_with('#') {
            custom = format!(".markdown-body {key}");
            custom_list = [custom.as_str()];
            &custom_list
        } else if let Some(selectors) = selector_map(key) {
            selectors
        } else {
            continue;
        };
        let mut text = String::new();
        for (property, value) in declarations {
            let property = camel_to_kebab(&property);
            // `cssValue ? ... : null`: an empty string declares nothing.
            if let Some(value) = to_css_value(&property, value).filter(|value| !value.is_empty()) {
                text.push_str(&property);
                text.push_str(": ");
                text.push_str(&value);
                text.push(';');
            }
        }
        if text.is_empty() {
            continue;
        }
        for selector in selectors {
            rules.push(format!("{selector} {{{text}}}"));
        }
    }
    rules.join("\n")
}

/// `selectorMap` (`XMarkdownAttributes.ts:395-421`). The selectors match the
/// element names the component's shadow tree uses, which are the HTML names
/// markdown-it renders (`h1`, `p`, `a`, `code`, `pre`, `ul`, `ol`, `li`,
/// `blockquote`, `table`, `thead`, `tbody`, `tr`, `th`, `td`, `span`).
///
/// The one difference is `image`: web-core writes `.markdown-body img`, but
/// an image in the shadow tree is the engine's own `image` element, so the
/// selector names `image`. web-core's `selectorMap[key]` also answers for
/// keys inherited from `Object.prototype` (`constructor`, `toString`, ...),
/// writing a rule whose selector is a function's source text or
/// `[object Object]`; no such rule is valid CSS, so those keys are skipped
/// here like any other unknown key.
fn selector_map(key: &str) -> Option<&'static [&'static str]> {
    Some(match key {
        "normalText" => &[".markdown-body"],
        "link" => &[".markdown-body a"],
        "inlineCode" => &[".markdown-body code:not(pre code)"],
        "codeBlock" => &[".markdown-body pre", ".markdown-body pre code"],
        "h1" => &[".markdown-body h1"],
        "h2" => &[".markdown-body h2"],
        "h3" => &[".markdown-body h3"],
        "h4" => &[".markdown-body h4"],
        "h5" => &[".markdown-body h5"],
        "h6" => &[".markdown-body h6"],
        "quote" => &[".markdown-body blockquote"],
        "orderedList" => &[".markdown-body ol"],
        "unorderedList" => &[".markdown-body ul"],
        "listItem" => &[".markdown-body li"],
        "image" => &[".markdown-body image"],
        "span" => &[".markdown-body span"],
        "p" => &[".markdown-body p"],
        "table" => &[".markdown-body table"],
        "thead" => &[".markdown-body thead"],
        "tbody" => &[".markdown-body tbody"],
        "tr" => &[".markdown-body tr"],
        "th" => &[".markdown-body th"],
        "td" => &[".markdown-body td"],
        "imageCaption" => &[".markdown-body .md-image-caption"],
        _ => return None,
    })
}

/// `unitlessCssProperties` (`XMarkdownAttributes.ts:385-393`): the
/// properties a bare number is written into without `px`.
const UNITLESS_PROPERTIES: [&str; 7] = [
    "font-weight",
    "opacity",
    "z-index",
    "flex",
    "flex-grow",
    "flex-shrink",
    "order",
];

/// `toCssValue` (`XMarkdownAttributes.ts:434-445`): a number gets `px`
/// unless the property is unitless, a string is verbatim except that a
/// property whose name contains `color` gets [`normalize_color`], and every
/// other value (null, boolean, object, array) declares nothing.
fn to_css_value(property: &str, value: &Json) -> Option<String> {
    match value {
        Json::Number(number) => {
            let number = js_number_to_string(*number);
            Some(if UNITLESS_PROPERTIES.contains(&property) {
                number
            } else {
                format!("{number}px")
            })
        }
        Json::String(text) if property.contains("color") => Some(normalize_color(text)),
        Json::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// `normalizeColor` (`XMarkdownAttributes.ts:423-429`): six to eight hex
/// digits, after trimming, get a `#`. The regex is `{6,8}`, so seven digits
/// count too.
fn normalize_color(value: &str) -> String {
    let hex = value.trim_matches(is_js_space);
    if (6..=8).contains(&hex.len()) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        format!("#{hex}")
    } else {
        value.to_owned()
    }
}

/// `camelToKebab` (`XMarkdownAttributes.ts:431-432`): every ASCII capital
/// becomes `-` and its lowercase.
fn camel_to_kebab(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 4);
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            out.push('-');
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

fn truncation(value: &Json) -> Option<Truncation> {
    let Json::Object(entries) = value else {
        return None;
    };
    let mut truncation = Truncation::default();
    for (key, value) in entries {
        match key.as_str() {
            "content" => truncation.content = js_truthy_string(value),
            "truncationType" => {
                truncation.truncation_type = match value {
                    Json::String(text) if text == "text" => Some(TruncationType::Text),
                    Json::String(text) if text == "view" => Some(TruncationType::View),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    Some(truncation)
}

fn typewriter_cursor(value: &Json) -> Option<TypewriterCursor> {
    let Json::Object(entries) = value else {
        return None;
    };
    let mut cursor = TypewriterCursor::default();
    for (key, value) in entries {
        let text = match value {
            Json::String(text) if !text.is_empty() => Some(text.clone()),
            _ => None,
        };
        match key.as_str() {
            "customCursor" => cursor.custom_cursor = text,
            "verticalAlign" => cursor.vertical_align = text,
            _ => {}
        }
    }
    Some(cursor)
}

/// `String(value)` of a truthy primitive, `None` for a falsy one.
fn js_truthy_string(value: &Json) -> Option<String> {
    match value {
        Json::String(text) if !text.is_empty() => Some(text.clone()),
        Json::Number(number) if *number != 0.0 && !number.is_nan() => {
            Some(js_number_to_string(*number))
        }
        Json::Bool(true) => Some("true".to_owned()),
        _ => None,
    }
}

/// A JSON value as `JSON.parse` builds it: objects keep their key order, and
/// a repeated key keeps the place of its first occurrence with the value of
/// its last.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_none<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "JSON.parse reads every number as a double"
    )]
    fn visit_i64<E>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Number(value as f64))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "JSON.parse reads every number as a double"
    )]
    fn visit_u64<E>(self, value: u64) -> Result<Json, E> {
        Ok(Json::Number(value as f64))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Json, E> {
        Ok(Json::Number(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Json>()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut entries: Vec<(String, Json)> = Vec::new();
        let mut index: FxHashMap<String, usize> = FxHashMap::default();
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            if let Some(&at) = index.get(&key) {
                entries[at].1 = value;
            } else {
                index.insert(key.clone(), entries.len());
                entries.push((key, value));
            }
        }
        Ok(Json::Object(entries))
    }
}

/// `Object.entries` order (ECMA-262 `OrdinaryOwnPropertyKeys`): the keys
/// that are array indices, ascending, then the others in insertion order.
fn js_entries(entries: &[(String, Json)]) -> Vec<(&str, &Json)> {
    let mut indices: Vec<(u32, &str, &Json)> = Vec::new();
    let mut others: Vec<(&str, &Json)> = Vec::new();
    for (key, value) in entries {
        match array_index(key) {
            Some(index) => indices.push((index, key, value)),
            None => others.push((key, value)),
        }
    }
    indices.sort_by_key(|&(index, ..)| index);
    indices
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .chain(others)
        .collect()
}

/// `Object.entries` of an array: its index keys, as text, in order.
fn array_entries(items: &[Json]) -> Vec<(Cow<'_, str>, &Json)> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| (Cow::Owned(index.to_string()), item))
        .collect()
}

/// A canonical array index: decimal digits without a leading zero (except
/// `"0"` itself), below 2³² − 1.
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty()
        || !key.bytes().all(|byte| byte.is_ascii_digit())
        || (key.len() > 1 && key.starts_with('0'))
    {
        return None;
    }
    key.parse::<u32>().ok().filter(|&index| index != u32::MAX)
}

/// ECMAScript `Number::toString`: the shortest digits that round-trip,
/// exponential below 10⁻⁶ and from 10²¹ on, and `0` for both zeros.
pub(super) fn js_number_to_string(number: f64) -> String {
    if number == 0.0 {
        return "0".to_owned();
    }
    if number.is_nan() {
        return "NaN".to_owned();
    }
    if number.is_infinite() {
        return if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_owned();
    }
    let magnitude = number.abs();
    if (1e-6..1e21).contains(&magnitude) {
        return format!("{number}");
    }
    // Rust writes `1e21` and `1.5e-7`; JavaScript writes the exponent's
    // sign either way.
    let text = format!("{number:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}e+{exponent}")
        }
        _ => text,
    }
}

/// ECMAScript's `WhiteSpace` and `LineTerminator`, the set `String.prototype.trim`
/// and the regex `\s` use: Unicode `White_Space` without U+0085, plus U+FEFF.
pub(super) fn is_js_space(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn css(value: &str) -> String {
        parse_markdown_style(value).css
    }

    /// The `style.html` fixture's attribute
    /// (`web-elements/tests/fixtures/x-markdown/style.html`), whose spec
    /// checks the link's `color: rgb(255, 0, 0)` and `line-height: 24px`
    /// and the inline code's color and background
    /// (`tests/x-markdown.spec.ts`, "should apply markdown-style updates").
    #[test]
    fn style_fixture() {
        let value = r#"{"normalText":{"fontSize":20},"link":{"color":"ff0000","lineHeight":24},"inlineCode":{"color":"0000ff","backgroundColor":"eeeeee"}}"#;
        assert_eq!(
            css(value),
            ".markdown-body {font-size: 20px;}\n\
             .markdown-body a {color: #ff0000;line-height: 24px;}\n\
             .markdown-body code:not(pre code) {color: #0000ff;background-color: #eeeeee;}"
        );
    }

    /// The same spec's update to `{ link: { color: '0000ff' } }`.
    #[test]
    fn style_update() {
        assert_eq!(
            css(r#"{"link":{"color":"0000ff"}}"#),
            ".markdown-body a {color: #0000ff;}"
        );
    }

    /// `html-tags.html`'s class keys.
    #[test]
    fn class_and_id_keys() {
        assert_eq!(
            css(r#"{".mark-red":{"color":"ff0000"},".mark-help":{"color":"00aa00"}}"#),
            ".markdown-body .mark-red {color: #ff0000;}\n.markdown-body .mark-help {color: #00aa00;}"
        );
    }

    /// Every expected string below is what web-core's own functions print
    /// for the same input, run under Node.
    #[test]
    fn values_and_selectors() {
        assert_eq!(
            css(
                r##"{"codeBlock":{"fontWeight":700,"opacity":0.5,"padding":null,"margin":true,"border":"","zIndex":-0},"unknown":{"color":"red"},"#id":{"borderColor":" abcdef0 ","background":"ff0000","color":"red"}}"##
            ),
            ".markdown-body pre {font-weight: 700;opacity: 0.5;z-index: 0;}\n\
             .markdown-body pre code {font-weight: 700;opacity: 0.5;z-index: 0;}\n\
             .markdown-body #id {border-color: #abcdef0;background: ff0000;color: red;}"
        );
    }

    /// Key order is `Object.entries`': array indices first, then insertion
    /// order; numbers are JavaScript's; `image` names the engine's element.
    #[test]
    fn key_order_and_numbers() {
        assert_eq!(
            css(
                r#"{"p":{"marginTop":4,"margin":0,"10":1,"2":2},"20":{"color":"red"},"image":{"width":1e21,"height":1.5e-7,"top":0.000001}}"#
            ),
            ".markdown-body p {2: 2px;10: 1px;margin-top: 4px;margin: 0px;}\n\
             .markdown-body image {width: 1e+21px;height: 1.5e-7px;top: 0.000001px;}"
        );
        // A repeated key keeps its first place and its last value.
        assert_eq!(
            css(r#"{"a":1,"a":2,"span":{"x":1,"y":2,"x":3}}"#),
            ".markdown-body span {x: 3px;y: 2px;}"
        );
    }

    #[test]
    fn configuration_keys() {
        let style = parse_markdown_style(
            r#"{"truncation":{"content":"Read more","truncationType":"view"},"typewriterCursor":{"customCursor":"none","verticalAlign":""},"h2":[1,2],"h3":"x","h4":{}}"#,
        );
        // An array's index entries are declarations; a string and an empty
        // object declare nothing.
        assert_eq!(style.css, ".markdown-body h2 {0: 1px;1: 2px;}");
        assert_eq!(
            style.truncation,
            Some(Truncation {
                content: Some("Read more".to_owned()),
                truncation_type: Some(TruncationType::View),
            })
        );
        assert_eq!(
            style.typewriter_cursor,
            Some(TypewriterCursor {
                custom_cursor: Some("none".to_owned()),
                vertical_align: None,
            })
        );
        let numeric =
            parse_markdown_style(r#"{"truncation":{"content":5},"typewriterCursor":"x"}"#);
        assert_eq!(
            numeric.truncation.and_then(|truncation| truncation.content),
            Some("5".to_owned())
        );
        assert_eq!(numeric.typewriter_cursor, None);
    }

    #[test]
    fn not_an_object() {
        for value in ["", "not json", "[1,2]", "3", "null", "\"a\""] {
            assert_eq!(
                parse_markdown_style(value),
                MarkdownStyle::default(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn number_text() {
        assert_eq!(js_number_to_string(24.0), "24");
        assert_eq!(js_number_to_string(-0.0), "0");
        assert_eq!(js_number_to_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(
            js_number_to_string(123_456_789_012_345_680_000.0),
            "123456789012345680000"
        );
    }
}
