//! `css/support/parsing-testcommon.js`, `computed-testcommon.js` and
//! `shorthand-testcommon.js`, over the engine's parser and cascade.
//!
//! - `test_valid_value` / `test_invalid_value`: the value is parsed as an inline declaration with
//!   the author `lynx` grammar, the way `element.style[property] = value` parses it, and read back
//!   with CSSOM specified-value serialization; a valid value must also round-trip.
//! - `test_computed_value`: the value is set as an inline declaration on the page's `#target`, the
//!   page is styled and laid out, and the property's resolved value is read as `getComputedStyle`
//!   reports it.
//! - `test_shorthand_value`: each listed longhand of the parsed shorthand serializes as expected
//!   and round-trips, and the shorthand sets no longhand outside the list.
//!
//! Each helper call is one subtest per WPT subtest it creates.

use stylo::context::QuirksMode;
use stylo::properties::{PropertyDeclarationBlock, PropertyId, parse_style_attribute};
use stylo::stylesheets::CssRuleType;

use super::check_layout::Subtest;
use super::loader::Page;
use super::script::{Call, Value};
use crate::common::url_data;

/// The helpers this module implements.
pub(crate) const HELPERS: &[&str] = &[
    "test_valid_value",
    "test_invalid_value",
    "test_computed_value",
    "test_shorthand_value",
];

fn hyphenate(property: &str) -> String {
    if property.starts_with("--") || !property.chars().any(|c| c.is_ascii_uppercase()) {
        return property.to_owned();
    }
    let mut out = String::new();
    for c in property.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    if out.starts_with("webkit-") {
        out.insert(0, '-');
    }
    out
}

fn block(property: &str, value: &str) -> Option<PropertyDeclarationBlock> {
    // CSSOM's setter rejects a value carrying its own priority or a
    // trailing declaration.
    if value.contains('!') || value.contains(';') {
        return None;
    }
    let block = parse_style_attribute(
        &format!("{property}: {value}"),
        &url_data(),
        None,
        QuirksMode::NoQuirks,
        CssRuleType::Style,
    );
    (!block.is_empty()).then_some(block)
}

fn specified(property: &str, value: &str) -> String {
    let Some(block) = block(property, value) else {
        return String::new();
    };
    let Ok(id) = PropertyId::parse_enabled_for_all_content(property) else {
        return String::new();
    };
    let mut out = String::new();
    let _ = block.property_value_to_css(&id, &mut out);
    out
}

fn expected_list(value: Option<&Value>, default: &str) -> Vec<String> {
    match value {
        None | Some(Value::Undefined | Value::Null) => vec![default.to_owned()],
        Some(Value::Str(text)) => vec![text.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Some(other) => vec![format!("{other:?}")],
    }
}

fn arg_str(call: &Call, index: usize) -> Result<&str, String> {
    call.args
        .get(index)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{}: argument {index} is not a string", call.name))
}

/// Runs one call; `None` for a call this module does not implement.
pub(crate) fn run(page: &mut Page, call: &Call) -> Option<Vec<Subtest>> {
    let outcome = match call.name.as_str() {
        "test_valid_value" => vec![valid(call)],
        "test_invalid_value" => vec![invalid(call)],
        "test_computed_value" => vec![computed(page, call)],
        "test_shorthand_value" => shorthand(call),
        _ => return None,
    };
    Some(outcome)
}

fn valid(call: &Call) -> Subtest {
    let property = hyphenate(arg_str(call, 0)?);
    let value = arg_str(call, 1)?;
    let expected = expected_list(call.args.get(2), value);
    let read = specified(&property, value);
    if read.is_empty() {
        return Err(format!("{property}: {value:?} does not parse"));
    }
    if !expected.contains(&read) {
        return Err(format!(
            "{property}: {value:?} serializes as {read:?}, expected {expected:?}"
        ));
    }
    let again = specified(&property, &read);
    if again != read {
        return Err(format!("{property}: {read:?} round-trips as {again:?}"));
    }
    Ok(())
}

fn invalid(call: &Call) -> Subtest {
    let property = hyphenate(arg_str(call, 0)?);
    let value = arg_str(call, 1)?;
    let read = specified(&property, value);
    if read.is_empty() {
        Ok(())
    } else {
        Err(format!("{property}: {value:?} parses as {read:?}"))
    }
}

fn computed_once(
    page: &mut Page,
    target: terminus::link::NodeId,
    property: &str,
    value: &str,
) -> Option<String> {
    page.dom.set_inline_style(target, "");
    page.dom.set_inline_style_property(target, property, value);
    page.dom.layout();
    page.dom.computed_style_text(target, property, true)
}

fn computed(page: &mut Page, call: &Call) -> Subtest {
    let property = hyphenate(arg_str(call, 0)?);
    let value = arg_str(call, 1)?;
    let expected = expected_list(call.args.get(2), value);
    let root = page.dom.document_element().id();
    let target = page
        .dom
        .query_selector(root, "#target")
        .ok()
        .flatten()
        .ok_or("no #target")?;
    if specified(&property, value).is_empty() {
        return Err(format!("{property}: {value:?} is not supported"));
    }
    let Some(read) = computed_once(page, target, &property, value) else {
        return Err(format!(
            "{property}: no computed value (shorthand or unknown)"
        ));
    };
    if !expected.contains(&read) {
        return Err(format!(
            "{property}: {value:?} computes to {read:?}, expected {expected:?}"
        ));
    }
    if read != value {
        let again = computed_once(page, target, &property, &read).unwrap_or_default();
        if again != read {
            return Err(format!(
                "{property}: computed {read:?} round-trips as {again:?}"
            ));
        }
    }
    Ok(())
}

fn shorthand(call: &Call) -> Vec<Subtest> {
    let (Ok(property), Ok(value)) = (arg_str(call, 0), arg_str(call, 1)) else {
        return vec![Err("test_shorthand_value: bad arguments".into())];
    };
    let property = hyphenate(property);
    let Some(Value::Object(longhands)) = call.args.get(2) else {
        return vec![Err("test_shorthand_value: no longhand map".into())];
    };
    let mut longhands: Vec<(String, String)> = longhands
        .iter()
        .map(|(name, expected)| {
            (
                hyphenate(name),
                expected.as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    longhands.sort();
    let parsed = block(&property, value);
    let mut results = Vec::new();
    for (longhand, expected) in &longhands {
        let outcome = (|| {
            let block = parsed
                .as_ref()
                .ok_or(format!("{property}: {value:?} does not parse"))?;
            let id = PropertyId::parse_enabled_for_all_content(longhand)
                .map_err(|()| format!("unknown longhand {longhand}"))?;
            let mut read = String::new();
            let _ = block.property_value_to_css(&id, &mut read);
            if &read != expected {
                return Err(format!(
                    "{property}: {value:?} sets {longhand} to {read:?}, expected {expected:?}"
                ));
            }
            let again = specified(longhand, &read);
            if again != read {
                return Err(format!("{longhand}: {read:?} round-trips as {again:?}"));
            }
            Ok(())
        })();
        results.push(outcome);
    }
    let unrelated = (|| {
        let block = parsed
            .as_ref()
            .ok_or(format!("{property}: {value:?} does not parse"))?;
        for declaration in block.declarations() {
            let name = declaration.id().name().to_string();
            if !longhands.iter().any(|(longhand, _)| *longhand == name) {
                return Err(format!("{property}: {value:?} also sets {name}"));
            }
        }
        Ok(())
    })();
    results.push(unrelated);
    results
}
