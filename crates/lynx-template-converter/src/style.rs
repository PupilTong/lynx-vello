use std::collections::BTreeMap;

use lynx_template_decoder::style_info::{
    CssProperty, DeclarationBlock, ParsedDeclaration, Rule, RulePrelude, RuleType, Selector,
    SimpleSelector, SimpleSelectorType, StyleSheet,
};

use crate::ConvertError;
use crate::native::{LepusValue, Version, decode_value};
use crate::reader::Reader;
use crate::tokenize::value_tokens;

const FONT_FACE_TRAILER: u8 = 1;
const PLACEHOLDER_RECURSION_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CssOptions {
    pub(crate) target_version: Version,
    pub(crate) enable_css_parser: bool,
    pub(crate) enable_css_variable: bool,
    pub(crate) enable_css_selector: bool,
}

#[derive(Debug)]
struct NativeSelector {
    relation: u8,
    match_type: u8,
    is_last_in_selector_list: bool,
    is_last_in_tag_history: bool,
    tag_is_implicit: bool,
    value: String,
    extra: Option<SelectorExtra>,
}

#[derive(Debug)]
struct SelectorExtra {
    value: String,
    match_type: u8,
    bits: Vec<LepusValue>,
    attribute: String,
    argument: String,
    selector_list: Vec<NativeSelector>,
}

pub(crate) fn decode_fragment(
    bytes: &[u8],
    options: CssOptions,
) -> Result<(i32, StyleSheet), ConvertError> {
    if options.enable_css_parser {
        return Err(ConvertError::UnsupportedCss(
            "pre-parsed native CSS values do not retain their source token text".to_owned(),
        ));
    }
    let mut reader = Reader::new(bytes);
    let native_id = i32::from_le_bytes(reader.u32()?.to_le_bytes());
    let import_count_raw = reader.u32()?;
    let import_count = checked_count(&reader, import_count_raw, 4, "CSS import")?;
    let mut imports = Vec::with_capacity(import_count);
    for _ in 0..import_count {
        imports.push(reader.i32()?);
    }

    if !options.enable_css_selector {
        return Err(ConvertError::UnsupportedCss(
            "CSS fragment does not contain serialized selectors".to_owned(),
        ));
    }

    let selector_count_raw = reader.u32()?;
    let selector_count = checked_count(&reader, selector_count_raw, 4, "selector")?;
    let mut rules = Vec::with_capacity(selector_count);
    for _ in 0..selector_count {
        let flattened_size_raw = reader.u32()?;
        let flattened_size = checked_count(&reader, flattened_size_raw, 1, "flattened selector")?;
        if flattened_size == 0 {
            continue;
        }
        let mut native_selectors = Vec::with_capacity(flattened_size);
        for _ in 0..flattened_size {
            let value = decode_value(&mut reader, 0)?;
            native_selectors.push(decode_selector(&value)?);
        }
        let declarations = decode_parse_token(&mut reader, options)?;
        rules.push(Rule {
            rule_type: RuleType::Declaration,
            prelude: RulePrelude {
                selector_list: convert_selector_list(&native_selectors)?,
            },
            declaration_block: DeclarationBlock { declarations },
            nested_rules: Vec::new(),
        });
    }

    let combined_size = reader.u32()?;
    let style_rule_count = combined_size & 0xffff;
    let keyframes_count = combined_size >> 16;
    if style_rule_count != 0 {
        return Err(ConvertError::UnsupportedCss(format!(
            "selector-enabled fragment unexpectedly contains {style_rule_count} legacy style rules"
        )));
    }
    for _ in 0..keyframes_count {
        let name = reader.string("keyframes name")?;
        rules.push(decode_keyframes(&mut reader, options, name)?);
    }

    while !reader.is_empty() {
        if reader.remaining() < 5 {
            return Err(ConvertError::invalid(
                reader.position(),
                "truncated typed CSS trailer",
            ));
        }
        let trailer_type = reader.u8()?;
        if trailer_type != FONT_FACE_TRAILER {
            return Err(ConvertError::UnsupportedCss(format!(
                "unknown typed CSS trailer {trailer_type}"
            )));
        }
        let family_count_raw = reader.u32()?;
        let family_count = checked_count(&reader, family_count_raw, 4, "font-face family")?;
        for _ in 0..family_count {
            let rule_count = if options.target_version.supports_extended_font_face() {
                let rule_count_raw = reader.u32()?;
                checked_count(&reader, rule_count_raw, 4, "font-face rule")?
            } else {
                1
            };
            for _ in 0..rule_count {
                rules.push(decode_font_face(&mut reader)?);
            }
        }
    }

    Ok((native_id, StyleSheet { imports, rules }))
}

fn decode_parse_token(
    reader: &mut Reader<'_>,
    options: CssOptions,
) -> Result<Vec<ParsedDeclaration>, ConvertError> {
    let mut declarations = decode_attributes(reader, options, false)?;
    if options.target_version.supports_important() {
        declarations.extend(decode_attributes(reader, options, true)?);
    }
    if options.target_version.supports_css_variables() && options.enable_css_variable {
        let count_raw = reader.u32()?;
        let count = checked_count(reader, count_raw, 8, "CSS variable")?;
        for _ in 0..count {
            let name = reader.string("CSS variable name")?;
            let value = reader.string("CSS variable value")?;
            declarations.push(make_declaration(
                CssProperty::from_name(name),
                &value,
                false,
            )?);
        }
    }
    Ok(declarations)
}

fn decode_attributes(
    reader: &mut Reader<'_>,
    options: CssOptions,
    important: bool,
) -> Result<Vec<ParsedDeclaration>, ConvertError> {
    let count_raw = reader.u32()?;
    let count = checked_count(reader, count_raw, 5, "CSS declaration")?;
    let mut declarations = Vec::with_capacity(count);
    for _ in 0..count {
        let property_id = reader.u32()?;
        let property = CssProperty::from_u32(property_id).ok_or_else(|| {
            ConvertError::UnsupportedCss(format!(
                "native property id {property_id} has no web StyleInfo equivalent"
            ))
        })?;
        if property.name().is_empty() {
            return Err(ConvertError::UnsupportedCss(
                "native declaration uses the invalid property id 0".to_owned(),
            ));
        }
        let value = decode_css_value(reader, options)?;
        declarations.push(make_declaration(property, &value, important)?);
    }
    Ok(declarations)
}

fn decode_css_value(reader: &mut Reader<'_>, options: CssOptions) -> Result<String, ConvertError> {
    if options.enable_css_parser {
        return Err(ConvertError::UnsupportedCss(
            "pre-parsed CSS values cannot be converted back to source tokens".to_owned(),
        ));
    }
    let value = decode_value(reader, 0)?;
    let mut defaults = BTreeMap::new();
    if options.target_version.supports_css_variables() && options.enable_css_variable {
        let _value_type = reader.u32()?;
        let _default_value = reader.string("CSS variable default")?;
        if options.target_version.supports_variable_default_map() {
            let default_map = decode_value(reader, 0)?;
            match default_map {
                LepusValue::Nil => {}
                LepusValue::Table(table) => {
                    for (name, value) in table {
                        let LepusValue::String(value) = value else {
                            return Err(ConvertError::UnsupportedCss(format!(
                                "CSS variable fallback {name:?} is not a string"
                            )));
                        };
                        defaults.insert(name, value);
                    }
                }
                _ => {
                    return Err(ConvertError::UnsupportedCss(
                        "CSS variable fallback map is not a table".to_owned(),
                    ));
                }
            }
        }
    }
    let source = css_value_text(value)?;
    restore_placeholders(&source, &defaults, 0)
}

fn css_value_text(value: LepusValue) -> Result<String, ConvertError> {
    match value {
        LepusValue::String(value) => Ok(value),
        LepusValue::Number(value) if value.is_finite() => Ok(value.to_string()),
        LepusValue::Bool(value) => Ok(value.to_string()),
        _ => Err(ConvertError::UnsupportedCss(
            "raw CSS declaration value is not scalar source text".to_owned(),
        )),
    }
}

fn restore_placeholders(
    source: &str,
    defaults: &BTreeMap<String, String>,
    depth: usize,
) -> Result<String, ConvertError> {
    if depth >= PLACEHOLDER_RECURSION_LIMIT {
        return Err(ConvertError::UnsupportedCss(
            "CSS variable fallback nesting limit exceeded".to_owned(),
        ));
    }
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("{{--") {
        output.push_str(&rest[..start]);
        let placeholder = &rest[start + 2..];
        let Some(end) = placeholder.find("}}") else {
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        let name = &placeholder[..end];
        if name.contains('}') {
            output.push_str(&rest[start..start + 2 + end + 2]);
        } else if let Some(fallback) = defaults.get(name) {
            let fallback = restore_placeholders(fallback, defaults, depth + 1)?;
            output.push_str("var(");
            output.push_str(name);
            output.push_str(", ");
            output.push_str(&fallback);
            output.push(')');
        } else {
            output.push_str("var(");
            output.push_str(name);
            output.push(')');
        }
        rest = &placeholder[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

fn make_declaration(
    property_id: CssProperty,
    value: &str,
    is_important: bool,
) -> Result<ParsedDeclaration, ConvertError> {
    Ok(ParsedDeclaration {
        property_id,
        value_token_list: value_tokens(value)?,
        is_important,
    })
}

fn decode_keyframes(
    reader: &mut Reader<'_>,
    options: CssOptions,
    name: String,
) -> Result<Rule, ConvertError> {
    let count_raw = reader.u32()?;
    let count = checked_count(reader, count_raw, 8, "keyframe")?;
    let mut nested_rules = Vec::with_capacity(count);
    for _ in 0..count {
        let key_text = reader.string("keyframe selector")?;
        let declarations = decode_attributes(reader, options, false)?;
        nested_rules.push(Rule {
            rule_type: RuleType::Declaration,
            prelude: unknown_text_prelude(key_text),
            declaration_block: DeclarationBlock { declarations },
            nested_rules: Vec::new(),
        });
    }
    Ok(Rule {
        rule_type: RuleType::KeyFrames,
        prelude: unknown_text_prelude(name),
        declaration_block: DeclarationBlock {
            declarations: Vec::new(),
        },
        nested_rules,
    })
}

fn decode_font_face(reader: &mut Reader<'_>) -> Result<Rule, ConvertError> {
    let count_raw = reader.u32()?;
    let count = checked_count(reader, count_raw, 8, "font-face declaration")?;
    let mut declarations = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader.string("font-face descriptor name")?;
        let value = reader.string("font-face descriptor value")?;
        declarations.push(make_declaration(
            CssProperty::from_name(name),
            &value,
            false,
        )?);
    }
    Ok(Rule {
        rule_type: RuleType::FontFace,
        prelude: RulePrelude {
            selector_list: Vec::new(),
        },
        declaration_block: DeclarationBlock { declarations },
        nested_rules: Vec::new(),
    })
}

fn unknown_text_prelude(value: String) -> RulePrelude {
    RulePrelude {
        selector_list: vec![Selector {
            simple_selectors: vec![SimpleSelector {
                selector_type: SimpleSelectorType::UnknownText,
                value,
            }],
        }],
    }
}

fn decode_selector(value: &LepusValue) -> Result<NativeSelector, ConvertError> {
    let array = value.array().ok_or_else(|| {
        ConvertError::UnsupportedCss("serialized selector is not a Lepus array".to_owned())
    })?;
    if array.len() != 3 {
        return Err(ConvertError::UnsupportedCss(format!(
            "serialized selector has {} fields instead of 3",
            array.len()
        )));
    }
    let bits = value_u32(&array[0], "selector flags")?;
    let _specificity = value_u32(&array[1], "selector specificity")?;
    let has_extra = bits & (1 << 18) != 0;
    let (value, extra) = if has_extra {
        (String::new(), Some(decode_selector_extra(&array[2])?))
    } else {
        (
            array[2]
                .string_value()
                .ok_or_else(|| {
                    ConvertError::UnsupportedCss(
                        "serialized selector value is not a string".to_owned(),
                    )
                })?
                .to_owned(),
            None,
        )
    };
    Ok(NativeSelector {
        relation: u8::try_from(bits & 0x0f).expect("four-bit relation"),
        match_type: u8::try_from((bits >> 4) & 0x0f).expect("four-bit match type"),
        is_last_in_selector_list: bits & (1 << 16) != 0,
        is_last_in_tag_history: bits & (1 << 17) != 0,
        tag_is_implicit: bits & (1 << 19) != 0,
        value,
        extra,
    })
}

fn decode_selector_extra(value: &LepusValue) -> Result<SelectorExtra, ConvertError> {
    let array = value.array().ok_or_else(|| {
        ConvertError::UnsupportedCss("selector extra data is not an array".to_owned())
    })?;
    if array.len() != 6 {
        return Err(ConvertError::UnsupportedCss(format!(
            "selector extra data has {} fields instead of 6",
            array.len()
        )));
    }
    let selector_list = match &array[5] {
        LepusValue::Array(values) => values
            .iter()
            .map(decode_selector)
            .collect::<Result<Vec<_>, _>>()?,
        LepusValue::Bool(false) | LepusValue::Nil => Vec::new(),
        _ => {
            return Err(ConvertError::UnsupportedCss(
                "nested selector list has an unexpected value type".to_owned(),
            ));
        }
    };
    Ok(SelectorExtra {
        value: required_string(&array[0], "selector extra value")?,
        match_type: u8::try_from(value_u32(&array[1], "selector extra match type")?)
            .map_err(|_| ConvertError::UnsupportedCss("selector match type overflow".to_owned()))?,
        bits: array[2]
            .array()
            .ok_or_else(|| {
                ConvertError::UnsupportedCss("selector extra bits are not an array".to_owned())
            })?
            .to_vec(),
        attribute: required_string(&array[3], "selector attribute")?,
        argument: required_string(&array[4], "selector argument")?,
        selector_list,
    })
}

fn convert_selector_list(selectors: &[NativeSelector]) -> Result<Vec<Selector>, ConvertError> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, selector) in selectors.iter().enumerate() {
        if selector.is_last_in_tag_history {
            result.push(convert_complex_selector(&selectors[start..=index])?);
            start = index + 1;
        }
    }
    if start != selectors.len() {
        return Err(ConvertError::UnsupportedCss(
            "serialized selector does not terminate its tag history".to_owned(),
        ));
    }
    if selectors
        .last()
        .is_some_and(|selector| !selector.is_last_in_selector_list)
    {
        return Err(ConvertError::UnsupportedCss(
            "serialized selector list has no final marker".to_owned(),
        ));
    }
    Ok(result)
}

fn convert_complex_selector(chain: &[NativeSelector]) -> Result<Selector, ConvertError> {
    let mut compounds: Vec<(&[NativeSelector], u8)> = Vec::new();
    let mut start = 0;
    for (index, selector) in chain.iter().enumerate() {
        if selector.relation != 0 || index + 1 == chain.len() {
            compounds.push((&chain[start..=index], selector.relation));
            start = index + 1;
        }
    }
    if start != chain.len() {
        return Err(ConvertError::UnsupportedCss(
            "selector compound boundaries are inconsistent".to_owned(),
        ));
    }

    let mut simple_selectors = Vec::new();
    for index in (0..compounds.len()).rev() {
        if index + 1 != compounds.len()
            && let Some(value) = combinator(compounds[index].1)?
        {
            simple_selectors.push(SimpleSelector {
                selector_type: SimpleSelectorType::Combinator,
                value: value.to_owned(),
            });
        }
        for selector in compounds[index].0 {
            if let Some(simple) = convert_simple_selector(selector)? {
                simple_selectors.push(simple);
            }
        }
    }
    Ok(Selector { simple_selectors })
}

fn combinator(relation: u8) -> Result<Option<&'static str>, ConvertError> {
    match relation {
        0 | 5 => Ok(None),
        1 | 6 => Ok(Some(" ")),
        2 | 7 => Ok(Some(">")),
        3 | 8 => Ok(Some("+")),
        4 | 9 => Ok(Some("~")),
        other => Err(ConvertError::UnsupportedCss(format!(
            "unknown selector relation {other}"
        ))),
    }
}

fn convert_simple_selector(
    selector: &NativeSelector,
) -> Result<Option<SimpleSelector>, ConvertError> {
    let value = selector
        .extra
        .as_ref()
        .map_or_else(|| selector.value.clone(), |extra| extra.value.clone());
    let simple = match selector.match_type {
        0 | 1 if selector.tag_is_implicit => return Ok(None),
        0 => {
            return Err(ConvertError::UnsupportedCss(
                "selector has an unknown match type".to_owned(),
            ));
        }
        1 if value == "*" => SimpleSelector {
            selector_type: SimpleSelectorType::UniversalSelector,
            value: String::new(),
        },
        1 => SimpleSelector {
            selector_type: SimpleSelectorType::TypeSelector,
            value,
        },
        2 => SimpleSelector {
            selector_type: SimpleSelectorType::IdSelector,
            value,
        },
        3 => SimpleSelector {
            selector_type: SimpleSelectorType::ClassSelector,
            value,
        },
        4 | 5 => {
            let value = format_pseudo(selector, value)?;
            SimpleSelector {
                selector_type: if selector.match_type == 4 {
                    SimpleSelectorType::PseudoClassSelector
                } else {
                    SimpleSelectorType::PseudoElementSelector
                },
                value,
            }
        }
        6..=12 => SimpleSelector {
            selector_type: SimpleSelectorType::AttributeSelector,
            value: format_attribute(selector)?,
        },
        other => {
            return Err(ConvertError::UnsupportedCss(format!(
                "unknown selector match type {other}"
            )));
        }
    };
    Ok(Some(simple))
}

fn format_pseudo(selector: &NativeSelector, name: String) -> Result<String, ConvertError> {
    let Some(extra) = selector.extra.as_ref() else {
        return Ok(name);
    };
    if !extra.selector_list.is_empty() {
        let nested = convert_selector_list(&extra.selector_list)?;
        let text = nested
            .iter()
            .map(Selector::to_css_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(format!("{name}({text})"));
    }
    if extra.match_type == 1 {
        if extra.bits.len() != 2 {
            return Err(ConvertError::UnsupportedCss(
                "nth pseudo-class does not have two coefficients".to_owned(),
            ));
        }
        let a = value_i32(&extra.bits[0], "nth coefficient")?;
        let b = value_i32(&extra.bits[1], "nth offset")?;
        return Ok(format!("{name}({})", format_an_plus_b(a, b)));
    }
    if !extra.argument.is_empty() {
        return Ok(format!("{name}({})", extra.argument));
    }
    Ok(name)
}

fn format_an_plus_b(a: i32, b: i32) -> String {
    if a == 0 {
        return b.to_string();
    }
    let mut result = match a {
        1 => "n".to_owned(),
        -1 => "-n".to_owned(),
        _ => format!("{a}n"),
    };
    if b > 0 {
        result.push('+');
        result.push_str(&b.to_string());
    } else if b < 0 {
        result.push_str(&b.to_string());
    }
    result
}

fn format_attribute(selector: &NativeSelector) -> Result<String, ConvertError> {
    let extra = selector.extra.as_ref().ok_or_else(|| {
        ConvertError::UnsupportedCss("attribute selector has no extra data".to_owned())
    })?;
    let operator = match selector.match_type {
        6 => "=",
        7 => "",
        8 => "|=",
        9 => "~=",
        10 => "*=",
        11 => "^=",
        12 => "$=",
        _ => unreachable!("caller limits attribute match types"),
    };
    if operator.is_empty() {
        return Ok(extra.attribute.clone());
    }
    let mut result = format!(
        "{}{}\"{}\"",
        extra.attribute,
        operator,
        escape_css_string(&extra.value)
    );
    if extra.match_type != 2 || extra.bits.len() != 2 {
        return Err(ConvertError::UnsupportedCss(
            "attribute selector has malformed extra data".to_owned(),
        ));
    }
    match value_u32(&extra.bits[0], "attribute case flag")? {
        0 => {}
        1 => result.push_str(" i"),
        2 => result.push_str(" s"),
        other => {
            return Err(ConvertError::UnsupportedCss(format!(
                "unknown attribute case flag {other}"
            )));
        }
    }
    Ok(result)
}

fn escape_css_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn value_u32(value: &LepusValue, context: &str) -> Result<u32, ConvertError> {
    let value = value.number_i32().ok_or_else(|| {
        ConvertError::UnsupportedCss(format!("{context} is not a 32-bit integer"))
    })?;
    u32::try_from(value).map_err(|_| ConvertError::UnsupportedCss(format!("{context} is negative")))
}

fn value_i32(value: &LepusValue, context: &str) -> Result<i32, ConvertError> {
    value
        .number_i32()
        .ok_or_else(|| ConvertError::UnsupportedCss(format!("{context} is not an integer")))
}

fn required_string(value: &LepusValue, context: &str) -> Result<String, ConvertError> {
    value
        .string_value()
        .map(str::to_owned)
        .ok_or_else(|| ConvertError::UnsupportedCss(format!("{context} is not a string")))
}

fn checked_count(
    reader: &Reader<'_>,
    raw: u32,
    minimum_bytes: usize,
    context: &str,
) -> Result<usize, ConvertError> {
    let count = usize::try_from(raw).map_err(|_| {
        ConvertError::invalid(reader.position(), format!("{context} count overflow"))
    })?;
    if count > reader.remaining() / minimum_bytes {
        return Err(ConvertError::invalid(
            reader.position(),
            format!("{context} count exceeds the remaining fragment"),
        ));
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_nested_variable_placeholders() {
        let defaults = BTreeMap::from([
            ("--a".to_owned(), "{{--b}}".to_owned()),
            ("--b".to_owned(), "10rpx".to_owned()),
        ]);
        assert_eq!(
            restore_placeholders("calc({{--a}} * 2)", &defaults, 0).unwrap(),
            "calc(var(--a, var(--b, 10rpx)) * 2)"
        );
    }

    #[test]
    fn formats_nth_coefficients() {
        assert_eq!(format_an_plus_b(2, 1), "2n+1");
        assert_eq!(format_an_plus_b(1, 0), "n");
        assert_eq!(format_an_plus_b(-1, 3), "-n+3");
        assert_eq!(format_an_plus_b(0, 4), "4");
    }
}
