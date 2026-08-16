//! Lowering a decoded `.web.bundle` `StyleInfo` into the engine's pre-parsed
//! stylesheet contract.
//!
//! Decoding the container is embedder work, so this conversion lives here
//! rather than in `bobcat-core`. It produces no stylesheet text: each rule
//! contributes its selector list and its declarations, and the engine parses
//! only those — never a re-serialized sheet.
//!
//! # Scope
//!
//! `css_id` scoping is deliberately not implemented. web-core keeps one CSS
//! fragment per `css_id` and guards each rule with a zero-specificity
//! `:where([l-css-id="N"])` derived from the import graph; here every
//! fragment's rules mount globally, which is what web-core itself produces
//! when the compiler emits `css_id` 0 (`enableRemoveCSSScope = true`) — the
//! shape every non-scoped bundle has. Fragments are still emitted in
//! **reverse-topological order**, imported before importing, so an importing
//! fragment's own rules win ties against the ones it imports; that is the
//! order web-core's TypeScript decoder used and what the C++ engine's
//! `ImportOtherFragment` means.
//!
//! web-core's browser-specific selector rewrites are also absent, because
//! nothing here needs them: Lynx tag names are this DOM's real element names
//! (no `view` → `x-view` mapping), the `page` element really is the document
//! element (so `:root` matches natively), and there is no entry-name
//! `:not([l-e-name])` guard because there is no shared browser document to
//! isolate one card from another.

use std::collections::{BTreeSet, HashMap, HashSet};

use bobcat_core::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};
use lynx_template_decoder::style_info::{
    DeclarationBlock, Rule, RuleKind, RulePrelude, StyleInfo, StyleSheet,
};

/// Lowers every fragment of a decoded `StyleInfo` into one pre-parsed sheet.
#[must_use]
pub(crate) fn to_preparsed_style_sheet(style_info: &StyleInfo) -> PreparsedStyleSheet {
    let mut rules = Vec::new();
    for css_id in fragment_order(style_info) {
        let sheet = &style_info.css_id_to_style_sheet[&css_id];
        rules.extend(sheet.rules.iter().filter_map(convert_rule));
    }
    PreparsedStyleSheet { rules }
}

/// Orders fragments so an imported fragment precedes every fragment importing
/// it, breaking ties by ascending `css_id` so the result never depends on hash
/// iteration order.
///
/// Fragments left in an import cycle are emitted last, in id order, rather
/// than dropped: web-core's Kahn pass silently loses them, and losing author
/// CSS is worse than an arbitrary but deterministic position.
fn fragment_order(style_info: &StyleInfo) -> Vec<i32> {
    let sheets = &style_info.css_id_to_style_sheet;
    let mut ids: Vec<i32> = sheets.keys().copied().collect();
    ids.sort_unstable();

    // What each fragment is still waiting on. Edges live in a set, so a
    // duplicated `@import` cannot strand its target the way web-core's
    // in-degree counter does.
    let mut blocking: HashMap<i32, BTreeSet<i32>> = ids
        .iter()
        .map(|id| (*id, live_imports(sheets, *id).collect()))
        .collect();

    let mut ordered = Vec::with_capacity(ids.len());
    let mut emitted: HashSet<i32> = HashSet::new();
    while let Some(id) = ids
        .iter()
        .copied()
        .find(|id| !emitted.contains(id) && blocking[id].is_empty())
    {
        emitted.insert(id);
        ordered.push(id);
        for waiting in blocking.values_mut() {
            waiting.remove(&id);
        }
    }
    ordered.extend(ids.into_iter().filter(|id| !emitted.contains(id)));
    ordered
}

/// The imports of `id` that name a fragment this bundle actually carries.
fn live_imports(sheets: &HashMap<i32, StyleSheet>, id: i32) -> impl Iterator<Item = i32> + '_ {
    sheets
        .get(&id)
        .into_iter()
        .flat_map(|sheet| sheet.imports.iter().copied())
        .filter(move |import| *import != id && sheets.contains_key(import))
}

fn convert_rule(rule: &Rule) -> Option<PreparsedRule> {
    match rule.kind {
        RuleKind::Style => Some(PreparsedRule::Style {
            selectors: selector_text(&rule.prelude)?,
            declarations: declarations(&rule.declaration_block),
        }),
        RuleKind::Keyframes => Some(PreparsedRule::Keyframes {
            // A keyframes prelude is a single `UnknownText` component holding
            // the animation name.
            name: selector_text(&rule.prelude)?,
            keyframes: rule
                .children
                .iter()
                .filter_map(|keyframe| {
                    Some(PreparsedKeyframe {
                        selector: selector_text(&keyframe.prelude)?,
                        declarations: declarations(&keyframe.declaration_block),
                    })
                })
                .collect(),
        }),
        // A font-face rule has an empty prelude by construction, and its
        // descriptors are not CSS properties.
        RuleKind::FontFace => Some(PreparsedRule::FontFace {
            descriptors: descriptor_text(&rule.declaration_block),
        }),
    }
}

/// The rule's selector list as CSS text, or `None` when it has no selectors.
fn selector_text(prelude: &RulePrelude) -> Option<String> {
    if prelude.selectors.is_empty() {
        return None;
    }
    let mut text = String::new();
    for (index, selector) in prelude.selectors.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&selector.to_css_string());
    }
    Some(text)
}

fn declarations(block: &DeclarationBlock) -> Vec<PreparsedDeclaration> {
    block
        .declarations
        .iter()
        .filter(|declaration| !declaration.property.name().is_empty())
        .map(|declaration| {
            // Value tokens carry their own delimiters, quotes and separating
            // whitespace, so concatenation reproduces the authored value.
            let value = declaration.value_text();
            let (value, important) = split_important(&value);
            PreparsedDeclaration {
                property: declaration.property.name().to_owned(),
                value: value.to_owned(),
                important: important || declaration.is_important,
            }
        })
        .collect()
}

/// Splits a trailing `!important` off a declaration value.
///
/// `ParsedDeclaration::is_important` is dead on the wire: web-core's encoder
/// hard-codes it to `false` and the CSS serializer that feeds it appends
/// ` !important` to the *value* string instead, so the marker arrives as
/// ordinary value tokens. Leaving it there would not merely lose the
/// importance — the value would fail to parse and the whole declaration would
/// be dropped, letting a later normal declaration win.
///
/// Only a genuine trailing marker is removed. A value ending in the bare ident
/// `important`, or in a string that happens to contain the word, is untouched
/// because the `!` is required.
fn split_important(value: &str) -> (&str, bool) {
    const MARKER: &str = "important";
    let trimmed = value.trim_end();
    let Some(head) = trimmed.len().checked_sub(MARKER.len()) else {
        return (value, false);
    };
    let Some(head) = trimmed.get(..head) else {
        return (value, false);
    };
    if !trimmed[head.len()..].eq_ignore_ascii_case(MARKER) {
        return (value, false);
    }
    // `!` and the keyword may be separated, and the keyword is ASCII-case
    // insensitive: `red ! IMPORTANT` is the same declaration as `red!important`.
    let Some(head) = head.trim_end().strip_suffix('!') else {
        return (value, false);
    };
    (head.trim_end(), true)
}

fn descriptor_text(block: &DeclarationBlock) -> String {
    let mut text = String::new();
    for declaration in &block.declarations {
        let name = declaration.property.name();
        if name.is_empty() {
            continue;
        }
        text.push_str(name);
        text.push(':');
        text.push_str(&declaration.value_text());
        text.push(';');
    }
    text
}

#[cfg(test)]
mod tests {
    use lynx_template_decoder::style_info::{
        CssProperty, CssPropertyId, ParsedDeclaration, Selector, SimpleSelector,
        SimpleSelectorKind, ValueToken, token_types,
    };

    use super::*;

    fn class_selector(name: &str) -> Selector {
        Selector {
            components: vec![SimpleSelector {
                kind: SimpleSelectorKind::Class,
                value: name.to_owned(),
            }],
        }
    }

    fn text_selector(text: &str) -> Selector {
        Selector {
            components: vec![SimpleSelector {
                kind: SimpleSelectorKind::UnknownText,
                value: text.to_owned(),
            }],
        }
    }

    fn declaration(id: CssPropertyId, value: &str) -> ParsedDeclaration {
        ParsedDeclaration {
            property: CssProperty {
                id,
                unknown_name: None,
            },
            value_tokens: vec![ValueToken {
                token_type: token_types::IDENT_TOKEN,
                value: value.to_owned(),
            }],
            is_important: false,
        }
    }

    fn style_rule(selectors: Vec<Selector>, declarations: Vec<ParsedDeclaration>) -> Rule {
        Rule {
            kind: RuleKind::Style,
            prelude: RulePrelude { selectors },
            declaration_block: DeclarationBlock { declarations },
            children: vec![],
        }
    }

    fn style_info(fragments: Vec<(i32, StyleSheet)>) -> StyleInfo {
        StyleInfo {
            css_id_to_style_sheet: fragments.into_iter().collect::<HashMap<_, _>>(),
            style_text_size_hint: 0,
        }
    }

    #[test]
    fn a_class_rule_becomes_a_preparsed_style_rule() {
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(
                    vec![class_selector("basic")],
                    vec![declaration(CssPropertyId::Color, "red")],
                )],
            },
        )]);

        let sheet = to_preparsed_style_sheet(&info);
        assert_eq!(
            sheet.rules,
            vec![PreparsedRule::Style {
                selectors: ".basic".to_owned(),
                declarations: vec![PreparsedDeclaration::new("color", "red")],
            }]
        );
    }

    #[test]
    fn a_selector_list_keeps_every_selector() {
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(
                    vec![class_selector("a"), class_selector("b")],
                    vec![declaration(CssPropertyId::Color, "red")],
                )],
            },
        )]);

        let PreparsedRule::Style { selectors, .. } = &to_preparsed_style_sheet(&info).rules[0]
        else {
            panic!("a style rule");
        };
        assert_eq!(selectors, ".a,.b");
    }

    #[test]
    fn multi_token_values_are_reassembled_verbatim() {
        let mut declaration = declaration(CssPropertyId::Border, "");
        declaration.value_tokens = vec![
            ValueToken {
                token_type: token_types::DIMENSION_TOKEN,
                value: "1px".to_owned(),
            },
            ValueToken {
                token_type: token_types::WHITESPACE_TOKEN,
                value: " ".to_owned(),
            },
            ValueToken {
                token_type: token_types::IDENT_TOKEN,
                value: "solid".to_owned(),
            },
            ValueToken {
                token_type: token_types::WHITESPACE_TOKEN,
                value: " ".to_owned(),
            },
            ValueToken {
                token_type: token_types::HASH_TOKEN,
                value: "#006400".to_owned(),
            },
        ];
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(vec![class_selector("a")], vec![declaration])],
            },
        )]);

        let PreparsedRule::Style { declarations, .. } = &to_preparsed_style_sheet(&info).rules[0]
        else {
            panic!("a style rule");
        };
        assert_eq!(declarations[0].value, "1px solid #006400");
    }

    #[test]
    fn keyframes_carry_their_name_and_child_blocks() {
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![Rule {
                    kind: RuleKind::Keyframes,
                    prelude: RulePrelude {
                        selectors: vec![text_selector("spin")],
                    },
                    declaration_block: DeclarationBlock {
                        declarations: vec![],
                    },
                    children: vec![
                        style_rule(
                            vec![text_selector("from")],
                            vec![declaration(CssPropertyId::Opacity, "0")],
                        ),
                        style_rule(
                            vec![text_selector("to")],
                            vec![declaration(CssPropertyId::Opacity, "1")],
                        ),
                    ],
                }],
            },
        )]);

        assert_eq!(
            to_preparsed_style_sheet(&info).rules,
            vec![PreparsedRule::Keyframes {
                name: "spin".to_owned(),
                keyframes: vec![
                    PreparsedKeyframe {
                        selector: "from".to_owned(),
                        declarations: vec![PreparsedDeclaration::new("opacity", "0")],
                    },
                    PreparsedKeyframe {
                        selector: "to".to_owned(),
                        declarations: vec![PreparsedDeclaration::new("opacity", "1")],
                    },
                ],
            }]
        );
    }

    #[test]
    fn a_font_face_rule_becomes_a_descriptor_block() {
        let mut src = declaration(CssPropertyId::Unknown, "url(a.ttf)");
        src.property.unknown_name = Some("src".to_owned());
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![Rule {
                    kind: RuleKind::FontFace,
                    prelude: RulePrelude { selectors: vec![] },
                    declaration_block: DeclarationBlock {
                        declarations: vec![declaration(CssPropertyId::FontFamily, "Custom"), src],
                    },
                    children: vec![],
                }],
            },
        )]);

        assert_eq!(
            to_preparsed_style_sheet(&info).rules,
            vec![PreparsedRule::FontFace {
                descriptors: "font-family:Custom;src:url(a.ttf);".to_owned(),
            }]
        );
    }

    #[test]
    fn a_declaration_with_no_property_name_is_dropped() {
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(
                    vec![class_selector("a")],
                    vec![
                        declaration(CssPropertyId::Unknown, "1"),
                        declaration(CssPropertyId::Color, "red"),
                    ],
                )],
            },
        )]);

        let PreparsedRule::Style { declarations, .. } = &to_preparsed_style_sheet(&info).rules[0]
        else {
            panic!("a style rule");
        };
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].property, "color");
    }

    /// The wire never sets `is_important`; the marker rides in the value.
    #[test]
    fn a_trailing_important_marker_becomes_the_importance_flag() {
        let cases = [
            ("red !important", "red", true),
            ("red!important", "red", true),
            ("red ! IMPORTANT", "red", true),
            ("0 auto !important", "0 auto", true),
            ("red", "red", false),
            // A bare `important` keyword is a value, not a marker.
            ("important", "important", false),
            // Strings keep their quotes, so the word inside one cannot be
            // mistaken for the marker.
            ("\"x !important\"", "\"x !important\"", false),
        ];
        for (input, value, important) in cases {
            assert_eq!(super::split_important(input), (value, important), "{input}");
        }
    }

    #[test]
    fn an_important_declaration_survives_lowering() {
        let mut important = declaration(CssPropertyId::Color, "");
        important.value_tokens = vec![
            ValueToken {
                token_type: token_types::IDENT_TOKEN,
                value: "red".to_owned(),
            },
            ValueToken {
                token_type: token_types::WHITESPACE_TOKEN,
                value: " ".to_owned(),
            },
            ValueToken {
                token_type: token_types::DELIM_TOKEN,
                value: "!".to_owned(),
            },
            ValueToken {
                token_type: token_types::IDENT_TOKEN,
                value: "important".to_owned(),
            },
        ];
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(vec![class_selector("a")], vec![important])],
            },
        )]);

        let PreparsedRule::Style { declarations, .. } = &to_preparsed_style_sheet(&info).rules[0]
        else {
            panic!("a style rule");
        };
        assert_eq!(
            declarations[0],
            PreparsedDeclaration::new("color", "red").important(true)
        );
    }

    /// An importing fragment's own rules must land after the ones it imports,
    /// so they win a specificity tie.
    #[test]
    fn fragments_emit_imported_before_importing() {
        let fragment = |imports: Vec<i32>, class: &str| StyleSheet {
            imports,
            rules: vec![style_rule(
                vec![class_selector(class)],
                vec![declaration(CssPropertyId::Color, class)],
            )],
        };
        let info = style_info(vec![
            (0, fragment(vec![1], "importer")),
            (1, fragment(vec![2], "middle")),
            (2, fragment(vec![], "base")),
        ]);

        let selectors: Vec<String> = to_preparsed_style_sheet(&info)
            .rules
            .into_iter()
            .map(|rule| match rule {
                PreparsedRule::Style { selectors, .. } => selectors,
                _ => panic!("style rules only"),
            })
            .collect();
        assert_eq!(selectors, [".base", ".middle", ".importer"]);
    }

    /// Fragment order must not depend on `HashMap` iteration order.
    #[test]
    fn independent_fragments_emit_in_id_order() {
        let fragment = |class: &str| StyleSheet {
            imports: vec![],
            rules: vec![style_rule(
                vec![class_selector(class)],
                vec![declaration(CssPropertyId::Color, "red")],
            )],
        };
        let info = style_info(vec![
            (7, fragment("seven")),
            (0, fragment("zero")),
            (3, fragment("three")),
        ]);

        let selectors: Vec<String> = to_preparsed_style_sheet(&info)
            .rules
            .into_iter()
            .map(|rule| match rule {
                PreparsedRule::Style { selectors, .. } => selectors,
                _ => panic!("style rules only"),
            })
            .collect();
        assert_eq!(selectors, [".zero", ".three", ".seven"]);
    }

    /// A fragment that only carries `@import` edges is normal in scoped
    /// bundles and contributes nothing itself.
    #[test]
    fn an_import_only_fragment_contributes_no_rules() {
        let info = style_info(vec![
            (
                2_032_114,
                StyleSheet {
                    imports: vec![0],
                    rules: vec![],
                },
            ),
            (
                0,
                StyleSheet {
                    imports: vec![],
                    rules: vec![style_rule(
                        vec![class_selector("a")],
                        vec![declaration(CssPropertyId::Color, "red")],
                    )],
                },
            ),
        ]);

        assert_eq!(to_preparsed_style_sheet(&info).rules.len(), 1);
    }

    /// web-core's flattening drops every fragment in a cycle; keeping them is
    /// strictly better and must still terminate.
    #[test]
    fn an_import_cycle_still_emits_every_fragment() {
        let fragment = |imports: Vec<i32>, class: &str| StyleSheet {
            imports,
            rules: vec![style_rule(
                vec![class_selector(class)],
                vec![declaration(CssPropertyId::Color, "red")],
            )],
        };
        let info = style_info(vec![
            (0, fragment(vec![1], "a")),
            (1, fragment(vec![0], "b")),
        ]);

        assert_eq!(to_preparsed_style_sheet(&info).rules.len(), 2);
    }

    /// A duplicated import edge must not strand its target, the way web-core's
    /// in-degree counter does.
    #[test]
    fn a_duplicated_import_edge_does_not_strand_a_fragment() {
        let info = style_info(vec![
            (
                0,
                StyleSheet {
                    imports: vec![1, 1],
                    rules: vec![style_rule(
                        vec![class_selector("importer")],
                        vec![declaration(CssPropertyId::Color, "red")],
                    )],
                },
            ),
            (
                1,
                StyleSheet {
                    imports: vec![],
                    rules: vec![style_rule(
                        vec![class_selector("base")],
                        vec![declaration(CssPropertyId::Color, "red")],
                    )],
                },
            ),
        ]);

        let selectors: Vec<String> = to_preparsed_style_sheet(&info)
            .rules
            .into_iter()
            .map(|rule| match rule {
                PreparsedRule::Style { selectors, .. } => selectors,
                _ => panic!("style rules only"),
            })
            .collect();
        assert_eq!(selectors, [".base", ".importer"]);
    }
}
