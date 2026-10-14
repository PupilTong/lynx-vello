//! Integration tests decoding real `.web.bundle` files produced by the
//! fixture workspace (see `packages/reactlynx-test-fixtures/README.md`).

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use bobcat_source::web::style_info::{RuleKind, Selector};
use bobcat_source::web::{DecodeError, decode};

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn decodes_card_with_css() {
    let template = decode(fixtures::fixture("basic-class-selector").page).unwrap();

    assert_eq!(template.version, 1);
    assert_eq!(template.config_str("cardType"), Some("react"));
    assert!(!template.config_flag("isLazy"));
    assert!(template.config_flag("enableFiberArch"));

    let root = &template.lepus_code["root"];
    assert!(root.contains("use strict"), "lepus root should be JS text");

    assert!(!template.manifest.is_empty());
    assert!(
        template
            .manifest
            .keys()
            .any(|k| std::path::Path::new(k).extension() == Some("js".as_ref())),
        "manifest keys should be JS paths, got {:?}",
        template.manifest.keys().collect::<Vec<_>>()
    );

    assert!(template.custom_sections.as_ref().unwrap().is_object());

    let style_info = template.style_info.as_ref().unwrap();
    assert_eq!(style_info.css_id_to_style_sheet.len(), 1);
    let sheet = &style_info.css_id_to_style_sheet[&0];
    assert!(sheet.imports.is_empty());
    assert_eq!(sheet.rules.len(), 1);
    let rule = &sheet.rules[0];
    assert_eq!(rule.kind, RuleKind::Style);
    assert_eq!(rule.children.len(), 0);
    let selectors: Vec<String> = rule
        .prelude
        .selectors
        .iter()
        .map(Selector::to_css_string)
        .collect();
    assert_eq!(selectors, [".basic"]);
    let declarations: Vec<String> = rule
        .declaration_block
        .declarations
        .iter()
        .map(|d| format!("{}:{}", d.property.name(), d.value_text()))
        .collect();
    assert_eq!(
        declarations,
        ["background-color:pink", "height:100px", "width:100px"]
    );
    assert!(
        rule.declaration_block
            .declarations
            .iter()
            .all(|d| !d.is_important),
        "Lynx never emits !important"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn decodes_card_with_empty_style_info() {
    let template = decode(fixtures::fixture("basic-bindtap").page).unwrap();

    assert_eq!(template.config_str("cardType"), Some("react"));
    assert!(!template.config_flag("isLazy"));
    assert!(template.lepus_code.contains_key("root"));

    let style_info = template.style_info.as_ref().unwrap();
    assert!(style_info.css_id_to_style_sheet.is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn decodes_large_style_info() {
    let template = decode(fixtures::fixture("basic-performance-large-css").page).unwrap();

    let style_info = template.style_info.as_ref().unwrap();
    let rule_count: usize = style_info
        .css_id_to_style_sheet
        .values()
        .map(|sheet| sheet.rules.len())
        .sum();
    assert!(
        rule_count > 100,
        "expected a large stylesheet, got {rule_count} rules"
    );

    for sheet in style_info.css_id_to_style_sheet.values() {
        for rule in &sheet.rules {
            for declaration in &rule.declaration_block.declarations {
                assert!(!declaration.property.name().is_empty());
                assert!(!declaration.value_text().is_empty());
            }
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rejects_bad_magic() {
    let err = decode(b"NOTABUNDLE__????").unwrap_err();
    assert!(matches!(err, DecodeError::BadMagic { .. }), "{err}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rejects_future_version() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&bobcat_source::web::MAGIC_0.to_le_bytes());
    bytes.extend_from_slice(&bobcat_source::web::MAGIC_1.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    let err = decode(&bytes).unwrap_err();
    assert!(matches!(err, DecodeError::UnsupportedVersion(2)), "{err}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rejects_truncated_section() {
    let bundle = fixtures::fixture("basic-class-selector").page;
    let err = decode(&bundle[..bundle.len() - 100]).unwrap_err();
    assert!(matches!(err, DecodeError::UnexpectedEof { .. }), "{err}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rejects_unknown_section_label() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&bobcat_source::web::MAGIC_0.to_le_bytes());
    bytes.extend_from_slice(&bobcat_source::web::MAGIC_1.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&99u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let err = decode(&bytes).unwrap_err();
    assert!(matches!(err, DecodeError::UnknownSection(99)), "{err}");
}

/// The selectors and declarations of a named section's one sheet, css id 0.
fn section_rules(style_info: &bobcat_source::web::StyleInfo) -> Vec<(String, Vec<String>)> {
    assert_eq!(
        style_info
            .css_id_to_style_sheet
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [0],
        "a CSS custom section is one sheet under css id 0"
    );
    style_info.css_id_to_style_sheet[&0]
        .rules
        .iter()
        .map(|rule| {
            (
                rule.prelude
                    .selectors
                    .iter()
                    .map(Selector::to_css_string)
                    .collect::<Vec<_>>()
                    .join(","),
                rule.declaration_block
                    .declarations
                    .iter()
                    .map(|d| format!("{}:{}", d.property.name(), d.value_text()))
                    .collect(),
            )
        })
        .collect()
}

/// `css-custom-sections` is a web card the canary encoder (lynx-stack#4138)
/// wrote with two `encoding: 'CSS'` custom sections: they arrive as the
/// `CustomSectionsStyleInfo` section (label 7), keyed by section name, and no
/// longer in the JSON `CustomSections` section.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn decodes_css_custom_sections_into_the_style_info_map() {
    let template = decode(fixtures::fixture("css-custom-sections").page).unwrap();

    assert_eq!(
        template
            .custom_sections_style_info
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["A &/中", "CSS"]
    );
    assert_eq!(
        section_rules(&template.custom_sections_style_info["CSS"]),
        [(
            ".from-css-section".to_owned(),
            vec!["width:80px".to_owned(), "background-color:blue".to_owned()]
        )]
    );
    assert_eq!(
        section_rules(&template.custom_sections_style_info["A &/中"]),
        [(
            ".from-named-section".to_owned(),
            vec![
                "height:40px".to_owned(),
                "background-color:green".to_owned()
            ]
        )]
    );

    let sections = template
        .custom_sections
        .as_ref()
        .unwrap()
        .as_object()
        .unwrap();
    assert!(
        sections
            .values()
            .all(|section| section.get("encoding").and_then(|e| e.as_str()) != Some("CSS")),
        "a CSS section is no longer JSON: {sections:?}"
    );
    // The card's own CSS stays where it was.
    let style_info = template.style_info.as_ref().unwrap();
    let selectors: Vec<String> = style_info.css_id_to_style_sheet[&0].rules[0]
        .prelude
        .selectors
        .iter()
        .map(Selector::to_css_string)
        .collect();
    assert_eq!(selectors, [".page"]);
}

/// The same container's named stylesheets as `ContainerSources` registers
/// them: each one under `custom_section/`, its name encoded, the `?#` suffix
/// kept.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn css_custom_sections_register_under_custom_section() {
    let url = url::Url::parse("https://cdn.test/sections.bundle?rev=1").unwrap();
    let sources = bobcat_source::ContainerSources::from_bytes(
        &url,
        fixtures::fixture("css-custom-sections").page,
    )
    .unwrap();
    // The card's own `StyleInfo` and its root hang under the container too.
    assert_eq!(
        sources.style_info().map(url::Url::as_str),
        Some("https://cdn.test/sections.bundle/style-info.css?rev=1")
    );
    assert_eq!(
        sources.entry().map(url::Url::as_str),
        Some("https://cdn.test/sections.bundle/root.js?rev=1")
    );
    let sheets: Vec<(&str, Vec<String>)> = sources
        .style_sheets()
        .iter()
        .map(|(url, sheet)| {
            (
                url.as_str(),
                sheet
                    .rules
                    .iter()
                    .map(|rule| match rule {
                        bobcat_core::PreparsedRule::Style { selectors, .. } => selectors.clone(),
                        other => panic!("a style rule, not {other:?}"),
                    })
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        sheets,
        [
            (
                "https://cdn.test/sections.bundle/custom_section/A%20%26%2F%E4%B8%AD.css?rev=1",
                vec![".from-named-section".to_owned()]
            ),
            (
                "https://cdn.test/sections.bundle/custom_section/CSS.css?rev=1",
                vec![".from-css-section".to_owned()]
            ),
        ]
    );
}

/// A label-7 payload that fails validation fails the whole decode, as a bad
/// `StyleInfo` does: no named sheet is half-loaded.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rejects_a_corrupt_custom_sections_style_info_section() {
    let mut bytes = fixtures::fixture("css-custom-sections").page.to_vec();
    // Walk to the label-7 section and overwrite the tail of its payload,
    // where an rkyv root-at-end archive keeps its root.
    let mut offset = 12;
    let (start, len) = loop {
        let label = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if label == bobcat_source::web::SectionLabel::CustomSectionsStyleInfo as u32 {
            break (offset + 8, len);
        }
        offset += 8 + len;
    };
    bytes[start + len - 16..start + len].fill(0xFF);
    let err = decode(&bytes).unwrap_err();
    assert!(
        matches!(err, DecodeError::CustomSectionsStyleInfo(_)),
        "{err}"
    );
}
