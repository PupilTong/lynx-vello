//! Lowering a decoded `.web.bundle` `StyleInfo` into the engine's pre-parsed
//! stylesheet contract.
//!
//! Decoding the container is embedder work, so this conversion lives here
//! rather than in `bobcat-core`. It produces no stylesheet text: each rule
//! contributes its selector list and its declarations, and the engine parses
//! only those — never a re-serialized sheet.
//!
//! # Fragments and `css_id` scoping
//!
//! A `StyleInfo` keeps one CSS fragment per `css_id`. The compiler puts every
//! rule outside an `@cssId` block into fragment 0 and makes each scoped
//! fragment `@import "0"` (lynx-stack
//! `packages/tools/css-serializer/src/css/debundle.ts:64-146`). Fragments are
//! emitted in **reverse-topological order**, imported before importing, so
//! an importing fragment's own rules win ties against the ones it imports;
//! that is the order web-core's TypeScript decoder used and what the C++
//! engine's `ImportOtherFragment` means.
//!
//! Each style rule is then scoped the way web-core's decoder scopes it (rule 6
//! of `style_info_decoder.rs:69-275`, over the `imported_by` sets of
//! `flattened_style_info.rs:21-98`, both under lynx-stack
//! `packages/web-platform/web-core/src/template/template_sections/style_info/`):
//! every selector is repeated once per fragment that imports the rule's
//! fragment, itself included, and the copy for fragment `N` carries
//! `:where([l-css-id="N"])` at the end of its subject compound, so it matches
//! only an element the Element PAPI's `__SetCSSId` marked with that id. All
//! copies stay in the rule's one selector list, so scoping moves no rule in
//! the cascade and, `:where()` being zero-specificity, changes no specificity
//! either. Only the subject element's id is tested: the ancestors a
//! combinator reaches are not guarded, as in web-core.
//!
//! One deliberate deviation in the text, not the matching: when fragment 0 is
//! among a rule's importers — every rule of fragment 0 itself, since each
//! scoped fragment imports it — web-core emits the unchanged selector *and*
//! one guarded copy per other importer. Each guarded copy matches a subset of
//! what the unchanged one matches, at the same specificity and cascade
//! position, so only the unchanged selector is emitted here. Nothing reads
//! the selector text back, so dropping the copies changes no computed style.
//!
//! Nothing here reads `enableRemoveCSSScope`; web-core does not either. A
//! bundle built with it carries only fragment 0, whose rules then lower
//! unchanged, and its script never calls `__SetCSSId`. `@keyframes` and
//! `@font-face` stay global, as web-core emits them verbatim.
//!
//! web-core's tag map is applied to three tags only: a `Type` component
//! `svg` is written as the document tag `lynx-svg`, `input` as `x-input` and
//! `textarea` as `x-textarea` ([`write_component`] over
//! `bobcat_core::document_tag`), as web-core's decoder writes `x-svg`,
//! `x-input` and `x-textarea`. web-core's other selector rewrites are absent,
//! because nothing here needs them: every other Lynx tag name is this DOM's
//! real element name (no `view` → `x-view` mapping), the `page` element really
//! is the document
//! element (so `:root` matches natively), and there is no entry-name
//! `:not([l-e-name])` guard. That guard separates a lazy bundle's own
//! `StyleInfo` from its host card's, and a lazy bundle's `StyleInfo` is not
//! mounted (`docs/tracking/deviations.md`).

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};

use bobcat_core::{
    PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet, document_tag,
};

use crate::web::css_property::DeclarationRead;
use crate::web::style_info::{
    ComponentRead, CssPropertyId, RuleKind, RuleRead, SelectorRead, SimpleSelector,
    SimpleSelectorKind, StyleInfoRead, StyleSheetRead,
};

const CSS_ID_ATTRIBUTE: &str = "l-css-id";

type RuleOf<S> = <<S as StyleInfoRead>::Sheet as StyleSheetRead>::Rule;

#[must_use]
pub(crate) fn to_preparsed_style_sheet(style_info: &impl StyleInfoRead) -> PreparsedStyleSheet {
    let mut importers = imported_by(style_info);
    let mut rules = Vec::new();
    for css_id in fragment_order(style_info) {
        let Some(sheet) = style_info.sheet(css_id) else {
            continue;
        };
        let scopes = scopes(importers.remove(&css_id).unwrap_or_default());
        rules.extend(
            sheet
                .rules()
                .iter()
                .filter_map(|rule| convert_rule(rule, &scopes)),
        );
    }
    PreparsedStyleSheet { rules }
}

fn fragment_order(style_info: &impl StyleInfoRead) -> Vec<i32> {
    let mut ids: Vec<i32> = style_info.sheets().map(|(id, _)| id).collect();
    ids.sort_unstable();

    let mut blocking: HashMap<i32, BTreeSet<i32>> = ids
        .iter()
        .map(|id| (*id, live_imports(style_info, *id).collect()))
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

fn imported_by(style_info: &impl StyleInfoRead) -> HashMap<i32, BTreeSet<i32>> {
    let mut importers: HashMap<i32, BTreeSet<i32>> = HashMap::new();
    for (importer, _) in style_info.sheets() {
        let mut reached = HashSet::new();
        let mut pending = vec![importer];
        while let Some(id) = pending.pop() {
            if reached.insert(id) {
                importers.entry(id).or_default().insert(importer);
                pending.extend(live_imports(style_info, id));
            }
        }
    }
    importers
}

fn scopes(importers: BTreeSet<i32>) -> Vec<Option<SimpleSelector>> {
    if importers.contains(&0) {
        return vec![None];
    }
    importers
        .into_iter()
        .map(|css_id| {
            Some(SimpleSelector {
                kind: SimpleSelectorKind::PseudoClass,
                value: format!("where([{CSS_ID_ATTRIBUTE}=\"{css_id}\"])"),
            })
        })
        .collect()
}

pub(crate) fn fragment_rules<S: StyleInfoRead>(
    style_info: &S,
    root: i32,
) -> impl Iterator<Item = &RuleOf<S>> {
    let mut reachable = HashSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if reachable.insert(id) {
            pending.extend(live_imports(style_info, id));
        }
    }
    fragment_order(style_info)
        .into_iter()
        .filter(move |id| reachable.contains(id))
        .filter_map(|id| style_info.sheet(id))
        .flat_map(|sheet| sheet.rules().iter())
}

fn live_imports(style_info: &impl StyleInfoRead, id: i32) -> impl Iterator<Item = i32> + '_ {
    style_info
        .sheet(id)
        .into_iter()
        .flat_map(|sheet| sheet.imports().iter().copied())
        .filter(move |import| *import != id && style_info.sheet(*import).is_some())
}

fn convert_rule(rule: &impl RuleRead, scopes: &[Option<SimpleSelector>]) -> Option<PreparsedRule> {
    match rule.kind() {
        RuleKind::Style => Some(PreparsedRule::Style {
            selectors: scoped_selector_text(rule.selectors(), scopes)?,
            declarations: declarations(rule.declarations()),
        }),
        RuleKind::Keyframes => Some(PreparsedRule::Keyframes {
            name: selector_text(rule.selectors())?,
            keyframes: rule
                .children()
                .iter()
                .filter_map(|keyframe| {
                    Some(PreparsedKeyframe {
                        selector: selector_text(keyframe.selectors())?,
                        declarations: declarations(keyframe.declarations()),
                    })
                })
                .collect(),
        }),
        RuleKind::FontFace => Some(PreparsedRule::FontFace {
            descriptors: descriptor_text(rule.declarations()),
        }),
    }
}

fn selector_text(selectors: &[impl SelectorRead]) -> Option<String> {
    scoped_selector_text(selectors, &[None])
}

fn scoped_selector_text(
    selectors: &[impl SelectorRead],
    scopes: &[Option<SimpleSelector>],
) -> Option<String> {
    if selectors.is_empty() {
        return None;
    }
    let len = |component: &dyn ComponentRead| match component.kind() {
        SimpleSelectorKind::Type => document_tag(component.value()).len(),
        _ => component.value().len() + 2,
    };
    let selector_len: usize = selectors
        .iter()
        .flat_map(SelectorRead::components)
        .map(|component| len(component))
        .sum();
    let guards: usize = scopes.iter().flatten().map(|guard| len(guard)).sum();
    let count = selectors.len();
    let mut text = String::with_capacity((selector_len + count) * scopes.len() + guards * count);
    let mut first = true;
    for selector in selectors {
        let at = guard_index(selector);
        for scope in scopes {
            if !first {
                text.push(',');
            }
            first = false;
            let (before, after) = selector.components().split_at(at);
            for component in before {
                write_component(component, &mut text);
            }
            if let Some(guard) = scope {
                ComponentRead::write_css_string(guard, &mut text);
            }
            for component in after {
                write_component(component, &mut text);
            }
        }
    }
    Some(text)
}

fn write_component(component: &impl ComponentRead, text: &mut String) {
    if component.kind() == SimpleSelectorKind::Type {
        text.push_str(document_tag(component.value()));
    } else {
        component.write_css_string(text);
    }
}

fn guard_index(selector: &impl SelectorRead) -> usize {
    selector
        .components()
        .iter()
        .rposition(|component| {
            matches!(
                component.kind(),
                SimpleSelectorKind::Class
                    | SimpleSelectorKind::Id
                    | SimpleSelectorKind::Attribute
                    | SimpleSelectorKind::Type
                    | SimpleSelectorKind::Universal
                    | SimpleSelectorKind::PseudoClass
            )
        })
        .map_or(0, |index| index + 1)
}

fn declarations(block: &[impl DeclarationRead]) -> Vec<PreparsedDeclaration> {
    block
        .iter()
        .filter(|declaration| !declaration.name().is_empty())
        .map(|declaration| {
            let (value, important) = declaration.value_and_importance();
            PreparsedDeclaration {
                property: match declaration.property_id() {
                    CssPropertyId::Unknown => Cow::Owned(declaration.name().to_owned()),
                    id => Cow::Borrowed(id.name()),
                },
                value,
                important,
            }
        })
        .collect()
}

fn descriptor_text(block: &[impl DeclarationRead]) -> String {
    let mut text = String::new();
    for declaration in block {
        let name = declaration.name();
        if name.is_empty() {
            continue;
        }
        text.push_str(name);
        text.push(':');
        declaration.write_value_text(&mut text);
        text.push(';');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::style_info::{
        CssProperty, CssPropertyId, DeclarationBlock, ParsedDeclaration, Rule, RulePrelude,
        Selector, SimpleSelector, SimpleSelectorKind, StyleInfo, StyleSheet, ValueToken,
        token_types,
    };

    fn preparsed(property: &str, value: &str) -> PreparsedDeclaration {
        PreparsedDeclaration {
            property: property.to_owned().into(),
            value: value.to_owned(),
            important: false,
        }
    }

    fn important(property: &str, value: &str) -> PreparsedDeclaration {
        PreparsedDeclaration {
            important: true,
            ..preparsed(property, value)
        }
    }

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
    fn named_fragment_includes_reachable_imports_without_adopting_other_sections() {
        let fragment = |id, imports| {
            (
                id,
                StyleSheet {
                    imports,
                    rules: vec![style_rule(vec![class_selector(&format!("s{id}"))], vec![])],
                },
            )
        };
        let info = style_info(vec![
            fragment(1, vec![2, 2, 99]),
            fragment(2, vec![3]),
            fragment(3, vec![]),
            fragment(4, vec![]),
        ]);
        let names = fragment_rules(&info, 1)
            .map(|rule| selector_text(&rule.prelude.selectors).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, [".s3", ".s2", ".s1"]);
        assert_eq!(fragment_rules(&info, 99).count(), 0);
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
                declarations: vec![preparsed("color", "red")],
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
                        declarations: vec![preparsed("opacity", "0")],
                    },
                    PreparsedKeyframe {
                        selector: "to".to_owned(),
                        declarations: vec![preparsed("opacity", "1")],
                    },
                ],
            }]
        );
    }

    #[test]
    fn a_range_keyframe_selector_is_lowered_verbatim() {
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![Rule {
                    kind: RuleKind::Keyframes,
                    prelude: RulePrelude {
                        selectors: vec![text_selector("reveal")],
                    },
                    declaration_block: DeclarationBlock {
                        declarations: vec![],
                    },
                    children: vec![
                        style_rule(
                            vec![text_selector("entry 0%")],
                            vec![declaration(CssPropertyId::Opacity, "0")],
                        ),
                        style_rule(
                            vec![text_selector("entry 100%, exit 0%")],
                            vec![declaration(CssPropertyId::Opacity, "1")],
                        ),
                    ],
                }],
            },
        )]);

        assert_eq!(
            to_preparsed_style_sheet(&info).rules,
            vec![PreparsedRule::Keyframes {
                name: "reveal".to_owned(),
                keyframes: vec![
                    PreparsedKeyframe {
                        selector: "entry 0%".to_owned(),
                        declarations: vec![preparsed("opacity", "0")],
                    },
                    PreparsedKeyframe {
                        selector: "entry 100%, exit 0%".to_owned(),
                        declarations: vec![preparsed("opacity", "1")],
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

    #[test]
    fn scroll_timeline_declarations_lower_unchanged() {
        let token = |token_type, value: &str| ValueToken {
            token_type,
            value: value.to_owned(),
        };
        let space = || token(token_types::WHITESPACE_TOKEN, " ");
        let named = |name: &str, value_tokens: Vec<ValueToken>| ParsedDeclaration {
            property: CssProperty::from_name(name),
            value_tokens,
            is_important: false,
        };
        let declarations = vec![
            named(
                "scroll-timeline-name",
                vec![token(token_types::IDENT_TOKEN, "--card")],
            ),
            named(
                "view-timeline",
                vec![
                    token(token_types::IDENT_TOKEN, "--item"),
                    space(),
                    token(token_types::IDENT_TOKEN, "block"),
                    space(),
                    token(token_types::DIMENSION_TOKEN, "10px"),
                ],
            ),
            named(
                "timeline-scope",
                vec![token(token_types::IDENT_TOKEN, "--card")],
            ),
            named(
                "animation-timeline",
                vec![
                    token(token_types::FUNCTION_TOKEN, "scroll("),
                    token(token_types::IDENT_TOKEN, "root"),
                    space(),
                    token(token_types::IDENT_TOKEN, "inline"),
                    token(token_types::RIGHT_PARENTHESES_TOKEN, ")"),
                ],
            ),
            named(
                "animation-range",
                vec![
                    token(token_types::IDENT_TOKEN, "entry"),
                    space(),
                    token(token_types::PERCENTAGE_TOKEN, "10%"),
                    space(),
                    token(token_types::IDENT_TOKEN, "exit"),
                    space(),
                    token(token_types::PERCENTAGE_TOKEN, "90%"),
                ],
            ),
        ];
        let info = style_info(vec![(
            0,
            StyleSheet {
                imports: vec![],
                rules: vec![style_rule(vec![class_selector("a")], declarations)],
            },
        )]);

        let PreparsedRule::Style { declarations, .. } = &to_preparsed_style_sheet(&info).rules[0]
        else {
            panic!("a style rule");
        };
        assert_eq!(
            declarations,
            &vec![
                preparsed("scroll-timeline-name", "--card"),
                preparsed("view-timeline", "--item block 10px"),
                preparsed("timeline-scope", "--card"),
                preparsed("animation-timeline", "scroll(root inline)"),
                preparsed("animation-range", "entry 10% exit 90%"),
            ]
        );
    }

    #[test]
    fn an_important_declaration_survives_lowering() {
        let mut marked = declaration(CssPropertyId::Color, "");
        marked.value_tokens = vec![
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
                rules: vec![style_rule(vec![class_selector("a")], vec![marked])],
            },
        )]);

        let PreparsedRule::Style { declarations, .. } = &to_preparsed_style_sheet(&info).rules[0]
        else {
            panic!("a style rule");
        };
        assert_eq!(declarations[0], important("color", "red"));
    }

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
        assert_eq!(
            selectors,
            [
                ".zero",
                r#".three:where([l-css-id="3"])"#,
                r#".seven:where([l-css-id="7"])"#,
            ]
        );
    }

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

    fn selector(components: &[(SimpleSelectorKind, &str)]) -> Selector {
        Selector {
            components: components
                .iter()
                .map(|(kind, value)| SimpleSelector {
                    kind: *kind,
                    value: (*value).to_owned(),
                })
                .collect(),
        }
    }

    fn sheet(imports: Vec<i32>, rules: Vec<Rule>) -> StyleSheet {
        StyleSheet { imports, rules }
    }

    fn red(selectors: Vec<Selector>) -> Rule {
        style_rule(selectors, vec![declaration(CssPropertyId::Color, "red")])
    }

    fn lowered(info: &StyleInfo) -> Vec<String> {
        to_preparsed_style_sheet(info)
            .rules
            .into_iter()
            .map(|rule| match rule {
                PreparsedRule::Style { selectors, .. } => selectors,
                PreparsedRule::Keyframes { name, keyframes } => format!(
                    "@keyframes {name} {}",
                    keyframes
                        .iter()
                        .map(|keyframe| keyframe.selector.as_str())
                        .collect::<Vec<_>>()
                        .join("|")
                ),
                PreparsedRule::FontFace { descriptors } => format!("@font-face {descriptors}"),
                other => panic!("this lowering builds no {other:?}"),
            })
            .collect()
    }

    fn scoped_once(components: &[(SimpleSelectorKind, &str)]) -> String {
        let info = style_info(vec![(
            1,
            sheet(vec![], vec![red(vec![selector(components)])]),
        )]);
        lowered(&info).remove(0)
    }

    #[test]
    fn importers_are_the_transitive_closure_plus_the_fragment_itself() {
        use SimpleSelectorKind::Class;
        let info = style_info(vec![
            (0, sheet(vec![], vec![red(vec![selector(&[(Class, "g")])])])),
            (1, sheet(vec![2, 0], vec![])),
            (2, sheet(vec![3], vec![])),
            (3, sheet(vec![], vec![red(vec![selector(&[(Class, "c")])])])),
        ]);
        assert_eq!(
            lowered(&info),
            [
                ".g",
                r#".c:where([l-css-id="1"]),.c:where([l-css-id="2"]),.c:where([l-css-id="3"])"#,
            ]
        );
    }

    #[test]
    fn the_guard_ends_the_subject_compound() {
        use SimpleSelectorKind::{Class, Combinator, Type};
        assert_eq!(
            scoped_once(&[(Class, "card"), (Combinator, ">"), (Type, "view")]),
            r#".card > view:where([l-css-id="1"])"#
        );
        assert_eq!(
            scoped_once(&[(Class, "a"), (Combinator, " "), (Class, "b")]),
            r#".a .b:where([l-css-id="1"])"#
        );
        assert_eq!(
            scoped_once(&[
                (Class, "a"),
                (Combinator, " "),
                (Class, "b"),
                (Combinator, "~"),
                (Type, "view"),
                (Class, "c"),
            ]),
            r#".a .b ~ view.c:where([l-css-id="1"])"#
        );
    }

    #[test]
    fn a_pseudo_class_stays_before_the_guard_and_a_pseudo_element_after_it() {
        use SimpleSelectorKind::{Class, PseudoClass, PseudoElement};
        assert_eq!(
            scoped_once(&[(Class, "a"), (PseudoClass, "hover")]),
            r#".a:hover:where([l-css-id="1"])"#
        );
        assert_eq!(
            scoped_once(&[
                (Class, "a"),
                (PseudoClass, "hover"),
                (PseudoElement, "after")
            ]),
            r#".a:hover:where([l-css-id="1"])::after"#
        );
    }

    #[test]
    fn a_bare_pseudo_element_subject_guards_the_compound_before_it() {
        use SimpleSelectorKind::{Class, Combinator, PseudoElement};
        assert_eq!(
            scoped_once(&[
                (Class, "a"),
                (Combinator, ">"),
                (PseudoElement, "placeholder")
            ]),
            r#".a:where([l-css-id="1"]) > ::placeholder"#
        );
    }

    #[test]
    fn universal_and_attribute_subjects_take_the_guard() {
        use SimpleSelectorKind::{Attribute, Type, Universal};
        assert_eq!(
            scoped_once(&[(Universal, "")]),
            r#"*:where([l-css-id="1"])"#
        );
        assert_eq!(
            scoped_once(&[(Attribute, "[type=submit]")]),
            r#"[type=submit]:where([l-css-id="1"])"#
        );
        assert_eq!(
            scoped_once(&[(Type, "view"), (Attribute, "[type=submit]")]),
            r#"view[type=submit]:where([l-css-id="1"])"#
        );
    }

    #[test]
    fn a_selector_list_repeats_each_selector_per_importer() {
        use SimpleSelectorKind::Class;
        let info = style_info(vec![
            (
                1,
                sheet(
                    vec![],
                    vec![red(vec![
                        selector(&[(Class, "a")]),
                        selector(&[(Class, "b")]),
                    ])],
                ),
            ),
            (2, sheet(vec![1], vec![])),
        ]);
        assert_eq!(
            lowered(&info),
            [concat!(
                r#".a:where([l-css-id="1"]),.a:where([l-css-id="2"]),"#,
                r#".b:where([l-css-id="1"]),.b:where([l-css-id="2"])"#
            )]
        );
    }

    #[test]
    fn an_import_cycle_scopes_each_member_for_both() {
        use SimpleSelectorKind::Class;
        let info = style_info(vec![
            (
                1,
                sheet(vec![2], vec![red(vec![selector(&[(Class, "a")])])]),
            ),
            (
                2,
                sheet(vec![1], vec![red(vec![selector(&[(Class, "b")])])]),
            ),
        ]);
        assert_eq!(
            lowered(&info),
            [
                r#".a:where([l-css-id="1"]),.a:where([l-css-id="2"])"#,
                r#".b:where([l-css-id="1"]),.b:where([l-css-id="2"])"#,
            ]
        );
    }

    #[test]
    fn a_fragment_importing_only_the_global_one() {
        use SimpleSelectorKind::Class;
        let info = style_info(vec![
            (0, sheet(vec![], vec![red(vec![selector(&[(Class, "g")])])])),
            (
                5,
                sheet(vec![0], vec![red(vec![selector(&[(Class, "x")])])]),
            ),
        ]);
        assert_eq!(lowered(&info), [".g", r#".x:where([l-css-id="5"])"#]);
    }

    #[test]
    fn keyframes_and_font_faces_stay_global() {
        let keyframes = Rule {
            kind: RuleKind::Keyframes,
            prelude: RulePrelude {
                selectors: vec![text_selector("spin")],
            },
            declaration_block: DeclarationBlock {
                declarations: vec![],
            },
            children: vec![style_rule(
                vec![text_selector("to")],
                vec![declaration(CssPropertyId::Opacity, "1")],
            )],
        };
        let font_face = Rule {
            kind: RuleKind::FontFace,
            prelude: RulePrelude { selectors: vec![] },
            declaration_block: DeclarationBlock {
                declarations: vec![declaration(CssPropertyId::FontFamily, "Custom")],
            },
            children: vec![],
        };
        let info = style_info(vec![
            (1, sheet(vec![], vec![keyframes, font_face])),
            (2, sheet(vec![1], vec![])),
        ]);
        assert_eq!(
            lowered(&info),
            ["@keyframes spin to", "@font-face font-family:Custom;"]
        );
    }

    fn unscoped(components: &[(SimpleSelectorKind, &str)]) -> String {
        let info = style_info(vec![(
            0,
            sheet(vec![], vec![red(vec![selector(components)])]),
        )]);
        lowered(&info).remove(0)
    }

    #[test]
    fn the_type_selector_svg_is_written_as_lynx_svg() {
        use SimpleSelectorKind::{Class, Combinator, PseudoClass, Type};
        assert_eq!(unscoped(&[(Type, "svg")]), "lynx-svg");
        assert_eq!(
            unscoped(&[(Type, "svg"), (Class, "icon"), (PseudoClass, "hover")]),
            "lynx-svg.icon:hover"
        );
        assert_eq!(
            unscoped(&[(Type, "view"), (Combinator, ">"), (Type, "svg")]),
            "view > lynx-svg"
        );
        assert_eq!(
            scoped_once(&[(Type, "svg"), (Combinator, " "), (Type, "svg")]),
            r#"lynx-svg lynx-svg:where([l-css-id="1"])"#
        );
    }

    #[test]
    fn the_type_selectors_input_and_textarea_are_written_as_the_fields() {
        use SimpleSelectorKind::{Class, Combinator, PseudoClass, Type};
        assert_eq!(unscoped(&[(Type, "input")]), "x-input");
        assert_eq!(unscoped(&[(Type, "textarea")]), "x-textarea");
        assert_eq!(
            unscoped(&[(Type, "view"), (Combinator, ">"), (Type, "input")]),
            "view > x-input"
        );
        assert_eq!(
            scoped_once(&[(Type, "textarea")]),
            r#"x-textarea:where([l-css-id="1"])"#
        );
        assert_eq!(unscoped(&[(Type, "x-input-ng")]), "x-input-ng");
        assert_eq!(unscoped(&[(Type, "x-textarea-ng")]), "x-textarea-ng");
        assert_eq!(unscoped(&[(Class, "input")]), ".input");
        assert_eq!(
            unscoped(&[(PseudoClass, "not(textarea)")]),
            ":not(textarea)"
        );
    }

    #[test]
    fn nothing_but_the_type_selector_svg_is_renamed() {
        use SimpleSelectorKind::{Attribute, Class, Id, PseudoClass, Type};
        assert_eq!(unscoped(&[(Class, "svg")]), ".svg");
        assert_eq!(unscoped(&[(Id, "svg")]), "#svg");
        assert_eq!(unscoped(&[(Attribute, "[svg]")]), "[svg]");
        assert_eq!(unscoped(&[(PseudoClass, "not(svg)")]), ":not(svg)");
        assert_eq!(unscoped(&[(Type, "x-svg")]), "x-svg");
        assert_eq!(unscoped(&[(Type, "view")]), "view");
    }
}
