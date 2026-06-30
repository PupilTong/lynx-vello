mod common;

use reactlynx_decoder::{
    DecodeError, Value, Version, decode_template,
    model::{AttributeBinding, ElementBuiltInTag, ElementTag},
};

#[test]
fn decodes_minimal_route_based_config_bundle() {
    let bytes = common::minimal_config_bundle(r#"{"name":"demo"}"#);
    let bundle = decode_template(&bytes).unwrap();

    assert_eq!(bundle.header.magic, common::QUICK_MAGIC);
    assert_eq!(bundle.header.total_size, bytes.len() as u32);
    assert_eq!(bundle.header.target_sdk, Version::parse("3.9.0"));
    assert_eq!(bundle.compile_options.target_sdk, Version::parse("3.9.0"));
    assert!(bundle.compile_options.enable_flexible_template);
    assert!(bundle.compile_options.enable_fiber_arch);
    assert_eq!(
        bundle.page_config.as_ref().unwrap().raw_json,
        r#"{"name":"demo"}"#
    );
}

#[test]
fn truncated_bundle_returns_error() {
    let mut bytes = common::minimal_config_bundle("{}");
    bytes.pop();

    assert!(matches!(
        decode_template(&bytes),
        Err(DecodeError::SizeMismatch { .. } | DecodeError::UnexpectedEof { .. })
    ));
}

#[test]
fn decodes_minimal_element_template_tree() {
    let bytes = common::minimal_element_template_bundle();
    let bundle = decode_template(&bytes).unwrap();

    let (template_id, roots) = &bundle.element_templates.templates[0];
    assert_eq!(*template_id, "_et_test");
    assert_eq!(roots.len(), 1);

    let root = &roots[0];
    assert_eq!(root.tag, ElementTag::Builtin(ElementBuiltInTag::View));
    assert_eq!(root.classes, vec!["container"]);
    assert_eq!(root.children.len(), 1);

    let text = &root.children[0];
    assert_eq!(text.tag, ElementTag::Builtin(ElementBuiltInTag::Text));
    assert_eq!(text.children.len(), 1);

    let raw_text = &text.children[0];
    assert_eq!(
        raw_text.tag,
        ElementTag::Builtin(ElementBuiltInTag::RawText)
    );
    assert_eq!(
        raw_text.attributes_array,
        vec![AttributeBinding::Static {
            key: "text",
            value: Value::Str("hello"),
        }]
    );
}

#[test]
fn raw_css_section_is_preserved_for_later_decoder_run() {
    let bytes = common::minimal_css_bundle();
    let bundle = decode_template(&bytes).unwrap();

    assert_eq!(bundle.raw_css, Some(&0u32.to_le_bytes()[..]));
}
