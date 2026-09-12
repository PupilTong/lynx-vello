//! The compiler's `encoding: CSS` custom-section JSON, shared by native
//! conversion and web bundles. Values lower directly to the core stylesheet
//! vocabulary; no stylesheet text is synthesized or reparsed.

use std::collections::BTreeMap;

use bobcat_core::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
use serde_json::{Value, json};

use crate::web::style_info::{DeclarationBlock, Rule, RuleKind, StyleInfo};

/// The native fragment has already been decoded. Preserve its name with the
/// same CSS descriptor the compiler supplies to the web encoder.
pub(crate) fn named_descriptors(
    style_info: &StyleInfo,
    names: BTreeMap<String, i32>,
) -> impl Iterator<Item = (String, Value)> + '_ {
    names.into_iter().map(|(name, id)| {
        let rules = crate::lower_style::fragment_rules(style_info, id);
        (name, json!({"encoding":"CSS", "content":{"ruleList": rules.map(rule_json).collect::<Vec<_>>()}}))
    })
}

fn text(value: String) -> Value {
    let mut text = json!({"loc":{"line":0,"column":0}});
    text["value"] = Value::String(value);
    text
}

fn style_json(block: &DeclarationBlock) -> Vec<Value> {
    block
        .declarations
        .iter()
        .map(|decl| {
            let (value, important) = decl.value_and_importance();
            let value = if important {
                format!("{value} !important")
            } else {
                value
            };
            json!({"name":decl.property.name(),"value":value,
            "keyLoc":{"line":0,"column":0},"valLoc":{"line":0,"column":0}})
        })
        .collect()
}

fn rule_json(rule: &Rule) -> Value {
    let prelude = || {
        rule.prelude
            .selectors
            .iter()
            .map(crate::web::style_info::Selector::to_css_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    match rule.kind {
        RuleKind::Style => {
            json!({"type":"StyleRule", "selectorText":text(prelude()), "variables":{}, "style":style_json(&rule.declaration_block)})
        }
        RuleKind::FontFace => {
            json!({"type":"FontFaceRule", "style":style_json(&rule.declaration_block)})
        }
        RuleKind::Keyframes => {
            json!({"type":"KeyframesRule", "name":text(prelude()), "styles":rule.children.iter().map(|child| {
            let key = child.prelude.selectors.iter().map(crate::web::style_info::Selector::to_css_string).collect::<Vec<_>>().join(",");
            json!({"keyText":text(key), "variables":{}, "style":style_json(&child.declaration_block)})
        }).collect::<Vec<_>>()})
        }
    }
}

/// Select named compiler CSS descriptors without making unrelated source
/// sections into stylesheets. Invalid descriptors remain un-loadable.
pub(crate) fn named_style_sheets(
    template: &crate::web::WebTemplate,
) -> BTreeMap<String, std::sync::Arc<PreparsedStyleSheet>> {
    template
        .custom_sections
        .as_ref()
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|sections| sections.iter())
        .filter(|(_, section)| section.get("encoding").and_then(Value::as_str) == Some("CSS"))
        .filter_map(|(name, section)| {
            Some((
                name.clone(),
                std::sync::Arc::new(decode(section.get("content")?)?),
            ))
        })
        .collect()
}

/// Invalid or unsupported CSS descriptors cannot produce a loadable sheet;
/// native `LoadStyleSheet` likewise returns null when CSS decoding fails.
pub(crate) fn decode(content: &Value) -> Option<PreparsedStyleSheet> {
    let rules = content
        .get("ruleList")?
        .as_array()?
        .iter()
        .map(|rule| {
            Some(match rule.get("type")?.as_str()? {
                "StyleRule" => PreparsedRule::Style {
                    selectors: rule.get("selectorText")?.get("value")?.as_str()?.to_owned(),
                    declarations: declarations(rule)?,
                },
                "FontFaceRule" => PreparsedRule::FontFace {
                    descriptors: declarations(rule)?.into_iter().fold(
                        String::new(),
                        |mut text, d| {
                            text.push_str(&d.property);
                            text.push(':');
                            text.push_str(&d.value);
                            if d.important {
                                text.push_str(" !important");
                            }
                            text.push(';');
                            text
                        },
                    ),
                },
                "KeyframesRule" => PreparsedRule::Keyframes {
                    name: rule.get("name")?.get("value")?.as_str()?.to_owned(),
                    keyframes: rule
                        .get("styles")?
                        .as_array()?
                        .iter()
                        .map(|frame| {
                            Some(PreparsedKeyframe {
                                selector: frame.get("keyText")?.get("value")?.as_str()?.to_owned(),
                                declarations: declarations(frame)?,
                            })
                        })
                        .collect::<Option<_>>()?,
                },
                _ => return None,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(PreparsedStyleSheet { rules })
}

fn declarations(rule: &Value) -> Option<Vec<PreparsedDeclaration>> {
    let mut result = rule
        .get("style")?
        .as_array()?
        .iter()
        .map(|decl| {
            let property = decl.get("name")?.as_str()?.to_owned();
            let value = decl.get("value")?.as_str()?;
            let value = if decl.get("type").and_then(Value::as_str) == Some("css_var") {
                let defaults = decl
                    .get("defaultValueMap")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|map| map.iter())
                    .map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                    .collect::<Option<BTreeMap<_, _>>>()?;
                crate::native::style::restore_placeholders(value, &defaults, 0).ok()?
            } else {
                value.to_owned()
            };
            Some(declaration(property, &value))
        })
        .collect::<Option<Vec<_>>>()?;
    if let Some(variables) = rule.get("variables") {
        for (property, value) in variables.as_object()? {
            result.push(declaration(property.clone(), value.as_str()?));
        }
    }
    Some(result)
}

fn declaration(property: String, source: &str) -> PreparsedDeclaration {
    let mut input = cssparser::ParserInput::new(source);
    let mut parser = cssparser::Parser::new(&mut input);
    let mut end = source.len();
    let mut important = false;
    while !parser.is_exhausted() {
        let start = parser.position();
        if parser.try_parse(cssparser::parse_important).is_ok() && parser.is_exhausted() {
            end = start.byte_index();
            important = true;
            break;
        }
        match parser.next_including_whitespace_and_comments() {
            Ok(
                cssparser::Token::Function(_)
                | cssparser::Token::ParenthesisBlock
                | cssparser::Token::SquareBracketBlock
                | cssparser::Token::CurlyBracketBlock,
            ) => {
                // Consume the whole block now: otherwise the next try_parse
                // skips it implicitly, leaving `start` inside the function.
                let _ = parser.parse_nested_block::<_, (), ()>(|nested| {
                    while nested.next().is_ok() {}
                    Ok(())
                });
            }
            Err(_) => break,
            _ => {}
        }
    }
    PreparsedDeclaration {
        property,
        value: source[..end].trim().to_owned(),
        important,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_variables_keep_fallbacks_and_token_level_importance() {
        let sheet = decode(&json!({"ruleList":[{
            "type":"StyleRule", "selectorText":{"value":".box"},
            "variables":{"--size":"12px !important"},
            "style":[
                {"name":"width", "type":"css_var", "value":"calc({{--size}} * 2) !/**/IMPORTANT /*tail*/",
                 "defaultValueMap":{"--size":"{{--fallback}}", "--fallback":"10px"}},
                {"name":"content", "value":"\"!important\""},
                {"name":"height", "value":"var(--x, !important)"}
            ]
        }]})).unwrap();
        let PreparsedRule::Style { declarations, .. } = &sheet.rules[0] else {
            panic!("style");
        };
        assert_eq!(
            declarations[0],
            PreparsedDeclaration {
                property: "width".into(),
                value: "calc(var(--size, var(--fallback, 10px)) * 2)".into(),
                important: true,
            }
        );
        assert_eq!(declarations[1].value, "\"!important\"");
        assert!(!declarations[1].important);
        assert!(!declarations[2].important);
        assert_eq!(
            declarations[3],
            PreparsedDeclaration {
                property: "--size".into(),
                value: "12px".into(),
                important: true,
            }
        );
    }

    #[test]
    fn keyframes_and_font_descriptors_preserve_their_separate_grammars() {
        let sheet = decode(&json!({"ruleList":[
            {"type":"KeyframesRule", "name":{"value":"pulse"}, "styles":[
                {"keyText":{"value":"from,50%"}, "variables":{}, "style":[{"name":"opacity", "value":"0.5"}]}
            ]},
            {"type":"FontFaceRule", "style":[
                {"name":"font-family", "value":"Demo"},
                {"name":"src", "value":"url(demo.woff2) !important"}
            ]}
        ]})).unwrap();
        let PreparsedRule::Keyframes { name, keyframes } = &sheet.rules[0] else {
            panic!("keyframes");
        };
        assert_eq!(name, "pulse");
        assert_eq!(keyframes[0].selector, "from,50%");
        assert_eq!(keyframes[0].declarations[0].value, "0.5");
        // Invalid !important font descriptors must reach Stylo unchanged so
        // that it rejects them, rather than accidentally accepting a src.
        assert_eq!(
            sheet.rules[1],
            PreparsedRule::FontFace {
                descriptors: "font-family:Demo;src:url(demo.woff2) !important;".into(),
            }
        );
    }

    #[test]
    fn malformed_or_unsupported_sections_never_become_partially_loadable_sheets() {
        for input in [
            json!(null),
            json!({"ruleList":[{"type":"MediaRule"}]}),
            json!({"ruleList":[{"type":"StyleRule", "selectorText":{"value":".box"}, "style":{}}]}),
            json!({"ruleList":[{"type":"StyleRule", "selectorText":{"value":".box"}, "style":[], "variables":{"--x":2}}]}),
        ] {
            assert!(decode(&input).is_none(), "{input}");
        }
        assert!(decode(&json!({"ruleList":[]})).unwrap().is_empty());
    }
}
