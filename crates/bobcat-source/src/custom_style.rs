//! Named stylesheets: the custom sections a compiler tagged `encoding: CSS`,
//! which a card loads by name through `__LoadStyleSheet`.
//!
//! A container carries them in one of three shapes, all lowered here to the
//! core stylesheet vocabulary; no stylesheet text is synthesized or reparsed:
//!
//! 1. the `.web.bundle` `CustomSectionsStyleInfo` section (label 7), one pre-parsed `StyleInfo` per
//!    section name — what web-core's encoder writes since lynx-stack#4138;
//! 2. **legacy**: the compiler's JSON descriptor (`{"encoding": "CSS", "content": {"ruleList":
//!    [...]}}`) inside the JSON `CustomSections` section, which is where encoders before
//!    lynx-stack#4138 left it. Kept so such bundles still load; it can go once no supported bundle
//!    is built by an older encoder (`docs/tracking/deviations.md`);
//! 3. a `.lynx.bundle`'s CSS custom sections, which `native::decode` puts in the same
//!    `custom_sections_style_info` map as shape 1.

use std::collections::BTreeMap;
use std::sync::Arc;

use bobcat_core::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
use serde_json::Value;

/// Every named stylesheet a template carries, by section name.
///
/// A name the `CustomSectionsStyleInfo` section carries wins over a legacy
/// JSON descriptor of the same name; an encoder writes a CSS section in only
/// one of the two. Each `StyleInfo` is lowered like the bundle's own: a
/// section holds css id 0 only, so no `l-css-id` guard appears, and a
/// hand-made section carrying other ids is scoped the way any fragment is.
/// Invalid legacy descriptors remain un-loadable.
pub(crate) fn named_style_sheets(
    template: &crate::web::WebTemplate,
) -> BTreeMap<String, Arc<PreparsedStyleSheet>> {
    let mut sheets: BTreeMap<String, Arc<PreparsedStyleSheet>> = template
        .custom_sections
        .as_ref()
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|sections| sections.iter())
        .filter(|(_, section)| section.get("encoding").and_then(Value::as_str) == Some("CSS"))
        .filter_map(|(name, section)| {
            Some((name.clone(), Arc::new(decode(section.get("content")?)?)))
        })
        .collect();
    sheets.extend(
        template
            .custom_sections_style_info
            .iter()
            .map(|(name, style_info)| {
                (
                    name.clone(),
                    Arc::new(crate::lower_style::to_preparsed_style_sheet(style_info)),
                )
            }),
    );
    sheets
}

/// Lowers one legacy JSON descriptor's `content`. Invalid or unsupported
/// descriptors cannot produce a loadable sheet; native `LoadStyleSheet`
/// likewise returns null when CSS decoding fails.
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
    let mut parser = cssparser::Parser::new(source);
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
    use serde_json::json;

    use super::*;
    use crate::web::style_info::{
        DeclarationBlock, Rule, RuleKind, RulePrelude, Selector, SimpleSelector,
        SimpleSelectorKind, StyleInfo, StyleSheet,
    };

    fn class_rule_info(class: &str) -> StyleInfo {
        StyleInfo {
            css_id_to_style_sheet: std::collections::HashMap::from([(
                0,
                StyleSheet {
                    imports: vec![],
                    rules: vec![Rule {
                        kind: RuleKind::Style,
                        prelude: RulePrelude {
                            selectors: vec![Selector {
                                components: vec![SimpleSelector {
                                    kind: SimpleSelectorKind::Class,
                                    value: class.to_owned(),
                                }],
                            }],
                        },
                        declaration_block: DeclarationBlock::default(),
                        children: vec![],
                    }],
                },
            )]),
            style_text_size_hint: 0,
        }
    }

    fn selectors(sheet: &PreparsedStyleSheet) -> Vec<&str> {
        sheet
            .rules
            .iter()
            .map(|rule| match rule {
                PreparsedRule::Style { selectors, .. } => selectors.as_str(),
                other => panic!("a style rule, not {other:?}"),
            })
            .collect()
    }

    /// The label-7 map and the legacy JSON descriptors both name sheets; the
    /// map wins a name both carry, and a JSON section of another encoding is
    /// no stylesheet.
    #[test]
    fn the_style_info_map_wins_over_a_legacy_descriptor_of_the_same_name() {
        let template = crate::web::WebTemplate {
            custom_sections: Some(json!({
            "CSS": {"encoding": "CSS", "content": {"ruleList": [
                {"type": "StyleRule", "selectorText": {"value": ".legacy"}, "style": []}
            ]}},
            "legacy-only": {"encoding": "CSS", "content": {"ruleList": [
                {"type": "StyleRule", "selectorText": {"value": ".old"}, "style": []}
            ]}},
            "background": {"content": "void 0"},
            })),
            custom_sections_style_info: BTreeMap::from([
                ("CSS".to_owned(), class_rule_info("current")),
                ("A &/中".to_owned(), class_rule_info("named")),
            ]),
            ..crate::web::WebTemplate::default()
        };
        let sheets = named_style_sheets(&template);
        assert_eq!(
            sheets.keys().map(String::as_str).collect::<Vec<_>>(),
            ["A &/中", "CSS", "legacy-only"]
        );
        assert_eq!(selectors(&sheets["CSS"]), [".current"]);
        assert_eq!(selectors(&sheets["A &/中"]), [".named"]);
        assert_eq!(selectors(&sheets["legacy-only"]), [".old"]);
    }

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
