//! Decoded `StyleInfo` → stylo, by **direct construction**.
//!
//! The `.web.bundle` wire format ships CSS pre-parsed (selector part lists +
//! per-declaration property ids and value text). This module lowers it
//! straight into stylo rule objects — one selector-list parse per rule, one
//! per-property value parse per declaration — with **no CSS-text
//! re-serialization of stylesheets** (web-core's approach for the browser).
//! See `docs/style-assumptions.md` §B.5.
//!
//! Lynx policy applied here (and deliberately NOT in `w3c-dom`):
//!
//! - **Import flattening** (web-core parity): each fragment's rules are emitted once per css id
//!   that transitively `@import`s it, in Kahn topological order; fragments on an import cycle are
//!   dropped, exactly like web-core's `FlattenedStyleInfo`.
//! - **cssId scoping**: every emitted selector variant gets a `:where([l-css-id="N"])` guard
//!   appended to its subject compound (zero specificity, so the author cascade is unperturbed)
//!   unless the importing css id is `0` (global; pageConfig `enableRemoveCSSScope` compiles to css
//!   id 0). String-parity with web-core's decoder output — same insertion anchor, same formatting.
//! - **No web-DOM rewrites**: web-core's `:root` → `[part="page"]`, `::placeholder` →
//!   `::part(input)::placeholder`, and `view` → `x-view` tag renames exist to map onto its browser
//!   host DOM. The native adapter records `<page>` as its own root and attaches it as an ordinary
//!   element beneath `w3c-dom`'s real document node; standard `:root` matching therefore remains a
//!   structural DOM concern rather than a `WidgetState` hook.
//! - **No entry-name guards** (`:not([l-e-name])`): lazy bundles are unsupported so far, so the
//!   guard could never exclude anything — it would be pure per-match overhead.
//! - The legacy `enableCSSSelector=false` (`css_og`) mode is **out of scope**
//!   (`docs/style-assumptions.md` §D.17): all Declaration rules are ingested as real CSS rules
//!   regardless of that page config.
//!
//! `@keyframes` stop lists and `@font-face` descriptor blocks are tiny and
//! rare compared to style rules; their *bodies* are reassembled to text and
//! parsed through stylo's dedicated sub-parsers (the rules themselves are
//! still built programmatically).

use lynx_template_decoder::style_info::{Rule, RuleType, Selector, SimpleSelectorType, StyleInfo};
use rustc_hash::{FxHashMap, FxHashSet};
use w3c_dom::CssRule;

/// One rule's declarations, lowered to `(name, value, important)` text parts.
fn lower_declarations(rule: &Rule) -> Vec<(&str, String, bool)> {
    rule.declaration_block
        .declarations
        .iter()
        .map(|declaration| {
            (
                declaration.property_id.name(),
                declaration.value_text(),
                declaration.is_important,
            )
        })
        .collect()
}

/// `css_id` → every css id whose fragment transitively imports it (including
/// itself), in web-core's flattening order. Fragments on an import cycle get
/// no entry (their rules are dropped, web-core parity).
fn imported_by_map(info: &StyleInfo) -> FxHashMap<i32, Vec<i32>> {
    // In-degree = number of fragments importing this one. A self-import
    // counts too (web-core parity: it makes the fragment a one-node cycle,
    // so it never becomes ready and drops with its import subtree).
    let mut in_degree: FxHashMap<i32, usize> = FxHashMap::default();
    for (&css_id, sheet) in &info.css_id_to_style_sheet {
        in_degree.entry(css_id).or_insert(0);
        for &import in &sheet.imports {
            if info.css_id_to_style_sheet.contains_key(&import) {
                *in_degree.entry(import).or_insert(0) += 1;
            }
        }
    }

    let mut imported_by: FxHashMap<i32, Vec<i32>> = FxHashMap::default();
    for &css_id in info.css_id_to_style_sheet.keys() {
        imported_by.insert(css_id, vec![css_id]);
    }

    // Kahn over the import DAG in a deterministic order (the wire format
    // stores fragments in a hash map, so no order is inherent). Only the
    // per-rule *variant* order depends on this; cascade order across rules
    // comes from the sorted emission loop in `build_rules`.
    let mut ready: Vec<i32> = in_degree
        .iter()
        .filter_map(|(&id, &degree)| (degree == 0).then_some(id))
        .collect();
    ready.sort_unstable();
    let mut processed: FxHashSet<i32> = FxHashSet::default();
    while let Some(css_id) = ready.pop() {
        processed.insert(css_id);
        let importers = imported_by.get(&css_id).cloned().unwrap_or_default();
        let Some(sheet) = info.css_id_to_style_sheet.get(&css_id) else {
            continue;
        };
        let mut newly_ready = Vec::new();
        for &import in &sheet.imports {
            // A self-importing fragment never becomes ready, so a self-edge
            // is unreachable here; the guard just keeps the loop total.
            if import == css_id || !info.css_id_to_style_sheet.contains_key(&import) {
                continue;
            }
            let entry = imported_by.entry(import).or_default();
            for &importer in &importers {
                if !entry.contains(&importer) {
                    entry.push(importer);
                }
            }
            let degree = in_degree.get_mut(&import).expect("edge counted above");
            *degree -= 1;
            if *degree == 0 {
                newly_ready.push(import);
            }
        }
        newly_ready.sort_unstable();
        ready.extend(newly_ready);
    }

    imported_by.retain(|css_id, _| processed.contains(css_id));
    imported_by
}

/// Append one selector variant, scoped to `scope_css_id` when `Some`,
/// mirroring web-core's part-by-part reassembly and its guard insertion
/// anchor (immediately after the subject compound, before trailing
/// pseudo-elements).
fn write_selector_variant(buf: &mut String, selector: &Selector, scope_css_id: Option<i32>) {
    // web-core: the anchor advances past every compound part
    // (class/id/attribute/type/universal/pseudo-class); combinators,
    // pseudo-elements, and unknown text do not advance it.
    let mut guard_at = 0;
    for (index, simple) in selector.simple_selectors.iter().enumerate() {
        match simple.selector_type {
            SimpleSelectorType::ClassSelector
            | SimpleSelectorType::IdSelector
            | SimpleSelectorType::AttributeSelector
            | SimpleSelectorType::TypeSelector
            | SimpleSelectorType::UniversalSelector
            | SimpleSelectorType::PseudoClassSelector => guard_at = index + 1,
            _ => {}
        }
    }

    for (index, simple) in selector.simple_selectors.iter().enumerate() {
        if index == guard_at
            && let Some(css_id) = scope_css_id
        {
            write_scope_guard(buf, css_id);
        }
        match simple.selector_type {
            SimpleSelectorType::TypeSelector | SimpleSelectorType::UnknownText => {
                buf.push_str(&simple.value);
            }
            SimpleSelectorType::ClassSelector => {
                buf.push('.');
                buf.push_str(&simple.value);
            }
            SimpleSelectorType::IdSelector => {
                buf.push('#');
                buf.push_str(&simple.value);
            }
            SimpleSelectorType::AttributeSelector => {
                buf.push('[');
                buf.push_str(&simple.value);
                buf.push(']');
            }
            SimpleSelectorType::PseudoClassSelector => {
                buf.push(':');
                buf.push_str(&simple.value);
            }
            SimpleSelectorType::PseudoElementSelector => {
                buf.push_str("::");
                buf.push_str(&simple.value);
            }
            SimpleSelectorType::UniversalSelector => {
                buf.push('*');
            }
            SimpleSelectorType::Combinator => {
                buf.push(' ');
                buf.push_str(&simple.value);
                buf.push(' ');
            }
        }
    }
    if guard_at == selector.simple_selectors.len()
        && let Some(css_id) = scope_css_id
    {
        write_scope_guard(buf, css_id);
    }
}

fn write_scope_guard(buf: &mut String, css_id: i32) {
    use std::fmt::Write;
    let _ = write!(buf, ":where([l-css-id=\"{css_id}\"])");
}

/// The comma-joined selector list for one style rule: one variant per
/// importing css id (unscoped for css id `0`), for each wire selector.
fn scoped_selector_list(rule: &Rule, imported_by: &[i32]) -> String {
    let mut buf = String::new();
    let mut first = true;
    for selector in &rule.prelude.selector_list {
        for &scope in imported_by {
            if !first {
                buf.push_str(", ");
            }
            first = false;
            let scope = (scope != 0).then_some(scope);
            write_selector_variant(&mut buf, selector, scope);
        }
    }
    buf
}

/// The `@keyframes` body text: `"<keytext> { decls } ..."`.
fn keyframes_body(rule: &Rule) -> String {
    let mut buf = String::new();
    for stop in &rule.nested_rules {
        let mut first = true;
        for selector in &stop.prelude.selector_list {
            if !first {
                buf.push_str(", ");
            }
            first = false;
            buf.push_str(&selector.to_css_string());
        }
        buf.push_str(" { ");
        write_declaration_block(&mut buf, stop);
        buf.push_str("} ");
    }
    buf
}

fn write_declaration_block(buf: &mut String, rule: &Rule) {
    for declaration in &rule.declaration_block.declarations {
        buf.push_str(declaration.property_id.name());
        buf.push_str(": ");
        buf.push_str(&declaration.value_text());
        if declaration.is_important {
            buf.push_str(" !important");
        }
        buf.push_str("; ");
    }
}

/// Lower every fragment of `info` into stylo rules via the engine's builders.
///
/// Returns the built rules; the caller mounts them
/// ([`StyleEngine::load_style_info`](crate::StyleEngine::load_style_info)).
pub(crate) fn build_rules(core: &w3c_dom::StyleEngine, info: &StyleInfo) -> Vec<CssRule> {
    let imported_by = imported_by_map(info);

    // Deterministic emission order: ascending css id. (The wire format's
    // hash map already destroyed cross-fragment source order; web-core
    // inherits its map's iteration order instead.)
    let mut css_ids: Vec<i32> = info.css_id_to_style_sheet.keys().copied().collect();
    css_ids.sort_unstable();

    let mut rules = Vec::new();
    for css_id in css_ids {
        let Some(importers) = imported_by.get(&css_id) else {
            // On an import cycle: dropped entirely (web-core parity).
            continue;
        };
        let sheet = &info.css_id_to_style_sheet[&css_id];
        for rule in &sheet.rules {
            let built = match rule.rule_type {
                RuleType::Declaration => {
                    let selectors = scoped_selector_list(rule, importers);
                    if selectors.is_empty() {
                        continue;
                    }
                    let declarations = lower_declarations(rule);
                    core.build_style_rule(
                        &selectors,
                        declarations
                            .iter()
                            .map(|(name, value, important)| (*name, value.as_str(), *important)),
                    )
                }
                RuleType::KeyFrames => {
                    let name = rule
                        .prelude
                        .selector_list
                        .first()
                        .map(Selector::to_css_string)
                        .unwrap_or_default();
                    core.build_keyframes_rule(name.trim(), &keyframes_body(rule))
                }
                RuleType::FontFace => {
                    let mut body = String::new();
                    write_declaration_block(&mut body, rule);
                    core.build_font_face_rule(&body)
                }
            };
            rules.extend(built);
        }
    }
    rules
}
