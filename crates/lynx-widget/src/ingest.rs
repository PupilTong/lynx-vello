//! Decoded `StyleInfo` → stylo, by **direct construction**.

use lynx_template_decoder::style_info::{Rule, RuleKind, Selector, SimpleSelectorKind, StyleInfo};
use rustc_hash::{FxHashMap, FxHashSet};
use w3c_dom::{CssDeclaration, CssRule};

fn lower_declarations(rule: &Rule) -> Vec<CssDeclaration<'_>> {
    rule.declaration_block
        .declarations
        .iter()
        .map(|declaration| CssDeclaration {
            property: declaration.property.name(),
            value: declaration.value_text().into(),
            important: declaration.is_important,
        })
        .collect()
}

fn imported_by_map(info: &StyleInfo) -> FxHashMap<i32, Vec<i32>> {
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

fn write_selector_variant(buf: &mut String, selector: &Selector, scope_css_id: Option<i32>) {
    let mut guard_at = 0;
    for (index, simple) in selector.components.iter().enumerate() {
        match simple.kind {
            SimpleSelectorKind::Class
            | SimpleSelectorKind::Id
            | SimpleSelectorKind::Attribute
            | SimpleSelectorKind::Type
            | SimpleSelectorKind::Universal
            | SimpleSelectorKind::PseudoClass => guard_at = index + 1,
            _ => {}
        }
    }

    for (index, simple) in selector.components.iter().enumerate() {
        if index == guard_at
            && let Some(css_id) = scope_css_id
        {
            write_scope_guard(buf, css_id);
        }
        match simple.kind {
            SimpleSelectorKind::Type | SimpleSelectorKind::UnknownText => {
                buf.push_str(&simple.value);
            }
            SimpleSelectorKind::Class => {
                buf.push('.');
                buf.push_str(&simple.value);
            }
            SimpleSelectorKind::Id => {
                buf.push('#');
                buf.push_str(&simple.value);
            }
            SimpleSelectorKind::Attribute => {
                buf.push('[');
                buf.push_str(&simple.value);
                buf.push(']');
            }
            SimpleSelectorKind::PseudoClass => {
                buf.push(':');
                buf.push_str(&simple.value);
            }
            SimpleSelectorKind::PseudoElement => {
                buf.push_str("::");
                buf.push_str(&simple.value);
            }
            SimpleSelectorKind::Universal => {
                buf.push('*');
            }
            SimpleSelectorKind::Combinator => {
                buf.push(' ');
                buf.push_str(&simple.value);
                buf.push(' ');
            }
        }
    }
    if guard_at == selector.components.len()
        && let Some(css_id) = scope_css_id
    {
        write_scope_guard(buf, css_id);
    }
}

fn write_scope_guard(buf: &mut String, css_id: i32) {
    use std::fmt::Write;
    let _ = write!(buf, ":where([l-css-id=\"{css_id}\"])");
}

fn scoped_selector_list(rule: &Rule, imported_by: &[i32]) -> String {
    let mut buf = String::new();
    let mut first = true;
    for selector in &rule.prelude.selectors {
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

fn keyframes_body(rule: &Rule) -> String {
    let mut buf = String::new();
    for stop in &rule.children {
        let mut first = true;
        for selector in &stop.prelude.selectors {
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
        buf.push_str(declaration.property.name());
        buf.push_str(": ");
        buf.push_str(&declaration.value_text());
        if declaration.is_important {
            buf.push_str(" !important");
        }
        buf.push_str("; ");
    }
}

pub(crate) fn build_rules<T>(document: &w3c_dom::Document<T>, info: &StyleInfo) -> Vec<CssRule> {
    let imported_by = imported_by_map(info);

    let mut css_ids: Vec<i32> = info.css_id_to_style_sheet.keys().copied().collect();
    css_ids.sort_unstable();

    let mut rules = Vec::new();
    for css_id in css_ids {
        let Some(importers) = imported_by.get(&css_id) else {
            continue;
        };
        let sheet = &info.css_id_to_style_sheet[&css_id];
        for rule in &sheet.rules {
            let built = match rule.kind {
                RuleKind::Style => {
                    let selectors = scoped_selector_list(rule, importers);
                    if selectors.is_empty() {
                        continue;
                    }
                    let declarations = lower_declarations(rule);
                    document.build_style_rule(&selectors, declarations)
                }
                RuleKind::Keyframes => {
                    let name = rule
                        .prelude
                        .selectors
                        .first()
                        .map(Selector::to_css_string)
                        .unwrap_or_default();
                    document.build_keyframes_rule(name.trim(), &keyframes_body(rule))
                }
                RuleKind::FontFace => {
                    let mut body = String::new();
                    write_declaration_block(&mut body, rule);
                    Some(document.build_font_face_rule(&body))
                }
            };
            rules.extend(built);
        }
    }
    rules
}
