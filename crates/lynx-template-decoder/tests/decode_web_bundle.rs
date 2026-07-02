//! Integration tests decoding real `.web.bundle` files produced by the
//! lynx-stack build pipeline (see `fixtures/README.md`).

use lynx_template_decoder::style_info::{RuleType, Selector};
use lynx_template_decoder::{DecodeError, decode};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

#[test]
fn decodes_lazy_component_with_css() {
    let template = decode(&fixture("config-lazy-component-css.web.bundle")).unwrap();

    assert_eq!(template.version, 1);
    assert_eq!(template.config_str("cardType"), Some("react"));
    assert!(template.config_flag("isLazy"));
    assert!(template.config_flag("enableFiberArch"));

    // Main-thread code is plain JavaScript, keyed by chunk name. The length
    // matches the reference decoder's `lepusCode.root.byteLength`.
    let root = &template.lepus_code["root"];
    assert_eq!(root.len(), 3068);
    assert!(root.contains("use strict"), "lepus root should be JS text");

    // Background-thread chunks.
    assert!(!template.manifest.is_empty());
    assert!(
        template
            .manifest
            .keys()
            .any(|k| std::path::Path::new(k).extension() == Some("js".as_ref())),
        "manifest keys should be JS paths, got {:?}",
        template.manifest.keys().collect::<Vec<_>>()
    );

    // The custom sections object exists (may be empty).
    assert!(template.custom_sections.as_ref().unwrap().is_object());

    // Real CSS. Expected values cross-validated against the reference
    // decoder (`@lynx-js/web-core` `decodeTemplate` + wasm), which flattens
    // this bundle to:
    //   .container[l-e-name=""]{background-color:orange;height:100px;width:100px;}
    let style_info = template.style_info.as_ref().unwrap();
    assert_eq!(style_info.css_id_to_style_sheet.len(), 1);
    let sheet = &style_info.css_id_to_style_sheet[&0];
    assert!(sheet.imports.is_empty());
    assert_eq!(sheet.rules.len(), 1);
    let rule = &sheet.rules[0];
    assert_eq!(rule.rule_type, RuleType::Declaration);
    assert_eq!(rule.nested_rules.len(), 0);
    let selectors: Vec<String> = rule
        .prelude
        .selector_list
        .iter()
        .map(Selector::to_css_string)
        .collect();
    assert_eq!(selectors, [".container"]);
    let declarations: Vec<String> = rule
        .declaration_block
        .declarations
        .iter()
        .map(|d| format!("{}:{}", d.property_id.name(), d.value_text()))
        .collect();
    assert_eq!(
        declarations,
        ["background-color:orange", "height:100px", "width:100px"]
    );
    assert!(
        rule.declaration_block
            .declarations
            .iter()
            .all(|d| !d.is_important),
        "Lynx never emits !important"
    );
}

#[test]
fn decodes_card_with_empty_style_info() {
    let template = decode(&fixture("basic-bindtap.web.bundle")).unwrap();

    assert_eq!(template.config_str("cardType"), Some("react"));
    assert!(!template.config_flag("isLazy"));
    assert!(template.lepus_code.contains_key("root"));

    // The section is present but its stylesheet map is empty.
    let style_info = template.style_info.as_ref().unwrap();
    assert!(style_info.css_id_to_style_sheet.is_empty());
}

#[test]
fn decodes_large_style_info() {
    let template = decode(&fixture("basic-performance-large-css.web.bundle")).unwrap();

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

    // Every interned property must resolve to a canonical name, and every
    // declaration's tokens must reassemble to non-empty text.
    for sheet in style_info.css_id_to_style_sheet.values() {
        for rule in &sheet.rules {
            for declaration in &rule.declaration_block.declarations {
                assert!(!declaration.property_id.name().is_empty());
                assert!(!declaration.value_text().is_empty());
            }
        }
    }
}

#[test]
fn rejects_bad_magic() {
    let err = decode(b"NOTABUNDLE__????").unwrap_err();
    assert!(matches!(err, DecodeError::BadMagic { .. }), "{err}");
}

#[test]
fn rejects_future_version() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&lynx_template_decoder::MAGIC_0.to_le_bytes());
    bytes.extend_from_slice(&lynx_template_decoder::MAGIC_1.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    let err = decode(&bytes).unwrap_err();
    assert!(matches!(err, DecodeError::UnsupportedVersion(2)), "{err}");
}

#[test]
fn rejects_truncated_section() {
    let bundle = fixture("config-lazy-component-css.web.bundle");
    let err = decode(&bundle[..bundle.len() - 100]).unwrap_err();
    assert!(matches!(err, DecodeError::UnexpectedEof { .. }), "{err}");
}

#[test]
fn rejects_unknown_section_label() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&lynx_template_decoder::MAGIC_0.to_le_bytes());
    bytes.extend_from_slice(&lynx_template_decoder::MAGIC_1.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&99u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let err = decode(&bytes).unwrap_err();
    assert!(matches!(err, DecodeError::UnknownSection(99)), "{err}");
}
