//! Re-serializing the decoded `StyleInfo` model as CSS text.
//!
//! The `StyleInfo` section is not CSS source: it is web-core's *pre-parsed*
//! form — selectors already split into simple selectors, declarations already
//! tokenized. A CSS engine that parses text (stylo, here) therefore needs the
//! model rendered back to text. That inverse of the decode is this module, and
//! it stays in this crate for the same reason [`Selector::to_css_string`] does:
//! it is the wire model's own serialization, not CSS-engine policy.
//!
//! # Value tokens concatenate without added separators
//!
//! [`ParsedDeclaration::value_text`] joins the value tokens with nothing
//! between them, and that is exact rather than approximate. The encoder emits a
//! whitespace token wherever whitespace was *significant* and drops it only
//! where the two neighbouring tokens re-tokenize unchanged without it — the
//! react example's `radial-gradient(… 62.3%at 46.43% …)` is a percentage token
//! followed by an ident token, and `box-shadow: … 0#ffd28db2` a number token
//! followed by a hash token. Both re-tokenize into the same two tokens with no
//! separator, and CSS grammars skip whitespace between component values, so
//! reintroducing spaces would be cosmetic rather than corrective.
//!
//! # Recorded limits
//!
//! - **CSS-scope ids are flattened.** Every decoded sheet is emitted, in `cssId` order, into one
//!   stylesheet. That is exactly right for a bundle built with `enableRemoveCSSScope` (which is
//!   what today's toolchain emits, and what the runtime has no `__SetCSSId` scope machinery for
//!   anyway) and wrong for a scoped one, where a rule would have to be restricted to the components
//!   carrying its `cssId`.
//! - **`imports` is not resolved.** Emitting every sheet already includes an imported sheet's
//!   rules; the import edge would only matter for ordering within a scoped cascade, which the
//!   previous limit already excludes.
//! - **Nested style rules are emitted as CSS nesting.** No encoder observed so far produces them
//!   (only `@keyframes` has children), so this path is untested against a real bundle.

use std::fmt::Write as _;

use crate::style_info::{Rule, RuleKind, StyleInfo, StyleSheet};

impl StyleInfo {
    /// Every decoded stylesheet as one CSS stylesheet, in `cssId` order.
    ///
    /// The result is author CSS text ready for a CSS engine; it is empty when
    /// the section carried no rules.
    #[must_use]
    pub fn to_css(&self) -> String {
        let mut css_ids: Vec<&i32> = self.css_id_to_style_sheet.keys().collect();
        css_ids.sort_unstable();
        let mut out = String::with_capacity(self.style_text_size_hint);
        for css_id in css_ids {
            self.css_id_to_style_sheet[css_id].write_css(&mut out);
        }
        out
    }
}

impl StyleSheet {
    /// This sheet's rules as CSS text.
    #[must_use]
    pub fn to_css(&self) -> String {
        let mut out = String::new();
        self.write_css(&mut out);
        out
    }

    fn write_css(&self, out: &mut String) {
        for rule in &self.rules {
            write_rule(rule, out);
        }
    }
}

fn write_rule(rule: &Rule, out: &mut String) {
    match rule.kind {
        RuleKind::Style => {
            let prelude = selector_list(rule);
            if prelude.is_empty() {
                return;
            }
            out.push_str(&prelude);
        }
        RuleKind::FontFace => out.push_str("@font-face"),
        RuleKind::Keyframes => {
            let name = selector_list(rule);
            if name.is_empty() {
                return;
            }
            let _ = write!(out, "@keyframes {name}");
        }
    }
    out.push_str("{\n");
    for declaration in &rule.declaration_block.declarations {
        let name = declaration.property.name();
        if name.is_empty() {
            continue;
        }
        let important = if declaration.is_important {
            " !important"
        } else {
            ""
        };
        let _ = writeln!(out, "  {name}:{}{important};", declaration.value_text());
    }
    for child in &rule.children {
        write_rule(child, out);
    }
    out.push_str("}\n");
}

/// A rule's prelude: its selectors, comma separated.
///
/// `@keyframes` reuses the selector list for its name, and a keyframe block
/// reuses it for its selector (`from`, `to`, `50%`) — both arrive as a single
/// `UnknownText` component, which serializes verbatim.
fn selector_list(rule: &Rule) -> String {
    rule.prelude
        .selectors
        .iter()
        .map(crate::style_info::Selector::to_css_string)
        .filter(|selector| !selector.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::css_property::{CssProperty, CssPropertyId, ParsedDeclaration, ValueToken};
    use crate::style_info::{
        DeclarationBlock, Rule, RuleKind, RulePrelude, Selector, SimpleSelector,
        SimpleSelectorKind, StyleInfo, StyleSheet,
    };

    fn declaration(name: &str, value: &str, important: bool) -> ParsedDeclaration {
        ParsedDeclaration {
            property: CssProperty {
                id: CssPropertyId::Unknown,
                unknown_name: Some(name.to_owned()),
            },
            value_tokens: vec![ValueToken {
                token_type: crate::style_info::token_types::IDENT_TOKEN,
                value: value.to_owned(),
            }],
            is_important: important,
        }
    }

    fn selector(kind: SimpleSelectorKind, value: &str) -> Selector {
        Selector {
            components: vec![SimpleSelector {
                kind,
                value: value.to_owned(),
            }],
        }
    }

    fn style_rule(selectors: Vec<Selector>, declarations: Vec<ParsedDeclaration>) -> Rule {
        Rule {
            kind: RuleKind::Style,
            prelude: RulePrelude { selectors },
            declaration_block: DeclarationBlock { declarations },
            children: Vec::new(),
        }
    }

    #[test]
    fn a_style_rule_becomes_a_selector_list_and_a_block() {
        let rule = style_rule(
            vec![
                selector(SimpleSelectorKind::Class, "Banner"),
                selector(SimpleSelectorKind::Class, "Logo"),
            ],
            vec![
                declaration("align-items", "center", false),
                declaration("display", "flex", true),
            ],
        );
        let sheet = StyleSheet {
            imports: Vec::new(),
            rules: vec![rule],
        };
        assert_eq!(
            sheet.to_css(),
            ".Banner,.Logo{\n  align-items:center;\n  display:flex !important;\n}\n"
        );
    }

    #[test]
    fn keyframes_wrap_their_children_as_keyframe_blocks() {
        let keyframes = Rule {
            kind: RuleKind::Keyframes,
            prelude: RulePrelude {
                selectors: vec![selector(SimpleSelectorKind::UnknownText, "Logo--spin")],
            },
            declaration_block: DeclarationBlock {
                declarations: Vec::new(),
            },
            children: vec![
                style_rule(
                    vec![selector(SimpleSelectorKind::UnknownText, "0%")],
                    vec![declaration("transform", "none", false)],
                ),
                style_rule(
                    vec![selector(SimpleSelectorKind::UnknownText, "to")],
                    vec![declaration("transform", "none", false)],
                ),
            ],
        };
        let sheet = StyleSheet {
            imports: Vec::new(),
            rules: vec![keyframes],
        };
        assert_eq!(
            sheet.to_css(),
            "@keyframes Logo--spin{\n0%{\n  transform:none;\n}\nto{\n  transform:none;\n}\n}\n"
        );
    }

    #[test]
    fn font_face_needs_no_prelude() {
        let sheet = StyleSheet {
            imports: Vec::new(),
            rules: vec![Rule {
                kind: RuleKind::FontFace,
                prelude: RulePrelude {
                    selectors: Vec::new(),
                },
                declaration_block: DeclarationBlock {
                    declarations: vec![declaration("font-family", "Lynx", false)],
                },
                children: Vec::new(),
            }],
        };
        assert_eq!(sheet.to_css(), "@font-face{\n  font-family:Lynx;\n}\n");
    }

    #[test]
    fn sheets_are_emitted_in_css_id_order() {
        let style_info = StyleInfo {
            css_id_to_style_sheet: HashMap::from([
                (
                    7,
                    StyleSheet {
                        imports: Vec::new(),
                        rules: vec![style_rule(
                            vec![selector(SimpleSelectorKind::Type, "view")],
                            vec![declaration("color", "red", false)],
                        )],
                    },
                ),
                (
                    0,
                    StyleSheet {
                        imports: Vec::new(),
                        rules: vec![style_rule(
                            vec![selector(SimpleSelectorKind::Type, "text")],
                            vec![declaration("color", "blue", false)],
                        )],
                    },
                ),
            ]),
            style_text_size_hint: 0,
        };
        let css = style_info.to_css();
        assert!(css.starts_with("text{"), "{css}");
        assert!(css.contains("view{"), "{css}");
        assert!(css.find("text{") < css.find("view{"), "{css}");
    }

    #[test]
    fn a_rule_with_no_usable_prelude_is_dropped() {
        let sheet = StyleSheet {
            imports: Vec::new(),
            rules: vec![style_rule(
                Vec::new(),
                vec![declaration("color", "red", false)],
            )],
        };
        assert_eq!(sheet.to_css(), "");
    }

    #[test]
    fn a_declaration_with_no_property_name_is_dropped() {
        let sheet = StyleSheet {
            imports: Vec::new(),
            rules: vec![style_rule(
                vec![selector(SimpleSelectorKind::Type, "view")],
                vec![ParsedDeclaration {
                    property: CssProperty {
                        id: CssPropertyId::Unknown,
                        unknown_name: None,
                    },
                    value_tokens: Vec::new(),
                    is_important: false,
                }],
            )],
        };
        assert_eq!(sheet.to_css(), "view{\n}\n");
    }
}
