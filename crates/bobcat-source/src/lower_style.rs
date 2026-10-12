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
//! web-core's other selector rewrites are absent, because nothing here needs
//! them: Lynx tag names are this DOM's real element names (no `view` →
//! `x-view` mapping), the `page` element really is the document element (so
//! `:root` matches natively), and there is no entry-name `:not([l-e-name])`
//! guard. That guard separates a lazy bundle's own `StyleInfo` from its host
//! card's, and a lazy bundle's `StyleInfo` is not mounted
//! (`docs/tracking/deviations.md`).

use std::collections::{BTreeSet, HashMap, HashSet};

use bobcat_core::{PreparsedDeclaration, PreparsedKeyframe, PreparsedRule, PreparsedStyleSheet};

use crate::web::style_info::{
    DeclarationBlock, Rule, RuleKind, RulePrelude, Selector, SimpleSelector, SimpleSelectorKind,
    StyleInfo, StyleSheet,
};

/// The attribute `__SetCSSId` writes; web-core's `CSS_ID_ATTRIBUTE`
/// (`packages/web-platform/web-core/src/constants.rs:9`).
const CSS_ID_ATTRIBUTE: &str = "l-css-id";

/// Lowers every fragment of a decoded `StyleInfo` into one pre-parsed sheet.
#[must_use]
pub(crate) fn to_preparsed_style_sheet(style_info: &StyleInfo) -> PreparsedStyleSheet {
    let mut importers = imported_by(style_info);
    let mut rules = Vec::new();
    for css_id in fragment_order(style_info) {
        let sheet = &style_info.css_id_to_style_sheet[&css_id];
        let scopes = scopes(importers.remove(&css_id).unwrap_or_default());
        rules.extend(
            sheet
                .rules
                .iter()
                .filter_map(|rule| convert_rule(rule, &scopes)),
        );
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

/// Every fragment's importers: the fragments that import it, directly or
/// through other fragments, and the fragment itself — web-core's
/// `imported_by` (`flattened_style_info.rs:66-78`). Ascending, so 0 first.
///
/// Built by walking down from each fragment rather than by web-core's
/// topological pass, so a fragment in an import cycle is not lost: the
/// members of a cycle each count the others as importers.
fn imported_by(style_info: &StyleInfo) -> HashMap<i32, BTreeSet<i32>> {
    let sheets = &style_info.css_id_to_style_sheet;
    let mut importers: HashMap<i32, BTreeSet<i32>> = HashMap::new();
    for &importer in sheets.keys() {
        let mut reached = HashSet::new();
        let mut pending = vec![importer];
        while let Some(id) = pending.pop() {
            if reached.insert(id) {
                importers.entry(id).or_default().insert(importer);
                pending.extend(live_imports(sheets, id));
            }
        }
    }
    importers
}

/// One fragment's importers as the components its selectors get: the
/// `:where([l-css-id="N"])` guard for each importing fragment `N`, or — when
/// fragment 0 is an importer — only `None`, the selector unchanged.
///
/// The unchanged selector matches every element, marked or not, so each
/// guarded copy beside it would match a strict subset of the same elements
/// with the same specificity (`:where()` adds none) in the same rule. Leaving
/// them out changes no match and no cascade outcome. It is a deliberate
/// string-level deviation from web-core, whose decoder emits those copies
/// (`style_info_decoder.rs:193-221`): for fragment 0 that is one copy per
/// scoped component, for every global selector.
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

/// A named fragment includes only its reachable imports, in the same cascade
/// order as the ordinary bundle stylesheet. Other named sections stay separate.
///
/// Its rules stay unscoped: a named section is loaded as a stylesheet of its
/// own, not mounted as the `css_id` fragment an element's `l-css-id` names.
pub(crate) fn fragment_rules(style_info: &StyleInfo, root: i32) -> impl Iterator<Item = &Rule> {
    let mut reachable = HashSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if reachable.insert(id) {
            pending.extend(live_imports(&style_info.css_id_to_style_sheet, id));
        }
    }
    fragment_order(style_info)
        .into_iter()
        .filter(move |id| reachable.contains(id))
        .flat_map(|id| style_info.css_id_to_style_sheet[&id].rules.iter())
}

/// The imports of `id` that name a fragment this bundle actually carries.
fn live_imports(sheets: &HashMap<i32, StyleSheet>, id: i32) -> impl Iterator<Item = i32> + '_ {
    sheets
        .get(&id)
        .into_iter()
        .flat_map(|sheet| sheet.imports.iter().copied())
        .filter(move |import| *import != id && sheets.contains_key(import))
}

/// `scopes` are the rule's fragment's [`scopes`]; only a style rule uses them.
fn convert_rule(rule: &Rule, scopes: &[Option<SimpleSelector>]) -> Option<PreparsedRule> {
    match rule.kind {
        RuleKind::Style => Some(PreparsedRule::Style {
            selectors: scoped_selector_text(&rule.prelude, scopes)?,
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
    scoped_selector_text(prelude, &[None])
}

/// The rule's selector list with every selector repeated once per scope, in
/// web-core's order — each selector's copies together, in `scopes` order —
/// or `None` when it has no selectors.
fn scoped_selector_text(
    prelude: &RulePrelude,
    scopes: &[Option<SimpleSelector>],
) -> Option<String> {
    if prelude.selectors.is_empty() {
        return None;
    }
    let mut text = String::new();
    let mut first = true;
    for selector in &prelude.selectors {
        let at = guard_index(selector);
        for scope in scopes {
            if !first {
                text.push(',');
            }
            first = false;
            match scope {
                None => selector.write_css_string(&mut text),
                Some(guard) => {
                    let (before, after) = selector.components.split_at(at);
                    for component in before {
                        component.write_css_string(&mut text);
                    }
                    guard.write_css_string(&mut text);
                    for component in after {
                        component.write_css_string(&mut text);
                    }
                }
            }
        }
    }
    Some(text)
}

/// Where a scope guard goes: just after the last component that is compound
/// content — class, id, attribute, type, universal or pseudo-class — which
/// puts it at the end of the subject compound and before a trailing
/// pseudo-element (`.a:hover:where(…)::after`). web-core's own index
/// (`style_info_decoder.rs:157-167`), with its quirk: a subject compound that
/// is nothing but a pseudo-element (`.a > ::placeholder`) has no content of
/// its own, so the guard lands on the compound before it, as it does there.
///
/// `UnknownText` does not move the index, as in web-core. Neither encoder
/// puts it in a style rule's selector — web-core's `encodeCSS.ts` pushes only
/// csstree node types there, and the native decoder emits it only for a
/// keyframes name or key (`native/style.rs`) — so what such text would hold is
/// unknown; it is not known to be compound content.
fn guard_index(selector: &Selector) -> usize {
    selector
        .components
        .iter()
        .rposition(|component| {
            matches!(
                component.kind,
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

fn declarations(block: &DeclarationBlock) -> Vec<PreparsedDeclaration> {
    block
        .declarations
        .iter()
        .filter(|declaration| !declaration.property.name().is_empty())
        .map(|declaration| {
            // The wire leaves `is_important` false and appends the marker to
            // the value instead, so the decoder splits it back out at token
            // level; handing the raw value to a CSS value parser would drop
            // the whole declaration.
            let (value, important) = declaration.value_and_importance();
            PreparsedDeclaration {
                property: declaration.property.name().to_owned(),
                value,
                important,
            }
        })
        .collect()
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
        // Descriptors are wire-faithful: `!important` is invalid in a
        // font-face block, so leaving it in the text makes stylo reject that
        // descriptor, which is what a browser does with the same input.
        declaration.write_value_text(&mut text);
        text.push(';');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::style_info::{
        CssProperty, CssPropertyId, ParsedDeclaration, Selector, SimpleSelector,
        SimpleSelectorKind, ValueToken, token_types,
    };

    fn preparsed(property: &str, value: &str) -> PreparsedDeclaration {
        PreparsedDeclaration {
            property: property.to_owned(),
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
            .map(|rule| selector_text(&rule.prelude).unwrap())
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

    /// web-core's encoder stores a keyframe block's selector as one
    /// `UnknownText` component holding the authored text, so a
    /// scroll-animations-1 range selector (`entry 0%`) reaches the engine's
    /// keyframe-selector parser as written.
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

    /// The scroll-animations-1 properties have no wire id, so a bundle names
    /// them as unknown properties; lowering hands their names and values to
    /// stylo as written.
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

    /// The wire leaves `is_important` false and puts the marker in the value,
    /// so lowering must recover it.
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
        // Fragment 0 imports 1 imports 2, so 0 reaches every rule and each
        // stays unguarded: a guarded copy would match a subset of it.
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
        assert_eq!(
            selectors,
            [
                ".zero",
                r#".three:where([l-css-id="3"])"#,
                r#".seven:where([l-css-id="7"])"#,
            ]
        );
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

    // -----------------------------------------------------------------------
    // `css_id` scoping
    // -----------------------------------------------------------------------

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

    /// Every lowered rule's selector text, keyframes and font-face included
    /// under a marker so a test can see they carry no guard.
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

    /// One selector in fragment 1, which nothing else imports.
    fn scoped_once(components: &[(SimpleSelectorKind, &str)]) -> String {
        let info = style_info(vec![(
            1,
            sheet(vec![], vec![red(vec![selector(components)])]),
        )]);
        lowered(&info).remove(0)
    }

    /// `imported_by` is transitive and includes the fragment itself: 1
    /// imports 2 imports 3, so 3's rule is copied for 1, 2 and 3 — and for no
    /// unscoped element, because fragment 0 does not reach it. Fragment 0,
    /// imported by 1, stays unguarded alone: its importers include 0, and a
    /// copy guarded for 1 would match a subset of the unguarded selector.
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

    /// Only the subject compound is guarded; the ancestors a combinator
    /// reaches are left alone, whatever the combinator.
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

    /// web-core's index counts only compound content, so a subject that is a
    /// bare pseudo-element puts the guard on the compound before it.
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
        // The native decoder's universal component carries no text.
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

    /// Each selector of a list keeps its copies together, in importer order,
    /// all in the one rule.
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

    /// The members of an import cycle count each other as importers; no
    /// fragment is dropped.
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

    /// The compiler's usual shape: a component fragment whose only import is
    /// the global fragment 0.
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

    /// `@keyframes` and `@font-face` are emitted verbatim, as web-core emits
    /// them, whatever fragment they are in.
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
}
