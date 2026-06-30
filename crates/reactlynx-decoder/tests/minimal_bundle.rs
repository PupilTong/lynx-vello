mod common;

use reactlynx_decoder::{
    DecodeError, Value, Version, decode_template,
    model::{
        AttributeBinding, CssFragmentBody, CssRule, CssValuePattern, ElementBuiltInTag, ElementTag,
    },
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
fn empty_css_section_decodes_to_empty_descriptor() {
    let bytes = common::minimal_css_bundle();
    let bundle = decode_template(&bytes).unwrap();

    assert!(bundle.css.fragments.is_empty());
}

#[test]
fn css_fragment_decodes_one_property_value() {
    let bytes = common::css_bundle_with_one_property(27, "12px");
    let bundle = decode_template(&bytes).unwrap();

    assert_eq!(bundle.css.fragments.len(), 1);
    let fragment = &bundle.css.fragments[0];
    assert_eq!(fragment.id, 7);
    let CssFragmentBody::Tokens(body) = &fragment.body else {
        panic!("expected token CSS body");
    };
    let (selector, token) = &body.tokens[0];
    assert_eq!(*selector, ".box");
    assert_eq!(token.attributes.len(), 1);
    assert_eq!(token.attributes[0].0, 27);
    assert_eq!(token.attributes[0].1.pattern, CssValuePattern::String);
    assert_eq!(token.attributes[0].1.value, Value::Str("12px"));
}

#[test]
fn truncated_css_fragment_returns_error() {
    let mut bytes = common::css_bundle_with_one_property(27, "12px");
    bytes.pop();
    let declared = bytes.len() as u32;
    bytes[0..4].copy_from_slice(&declared.to_le_bytes());

    assert!(decode_template(&bytes).is_err());
}

#[test]
fn parsed_styles_honor_effective_css_parser_gate() {
    let bytes = common::parsed_styles_bundle_without_css_pattern_byte(27, "12px");
    let bundle = decode_template(&bytes).unwrap();

    let parsed_styles = bundle.parsed_styles.as_ref().unwrap();
    let (key, block) = &parsed_styles.entries[0];
    assert_eq!(*key, "inline");
    assert_eq!(block.attributes.len(), 1);
    assert_eq!(block.attributes[0].0, 27);
    assert_eq!(block.attributes[0].1.pattern, CssValuePattern::String);
    assert_eq!(block.attributes[0].1.value, Value::Str("12px"));
    assert!(block.attributes[0].1.default_value.is_none());
}

#[test]
fn pre_2_7_css_font_face_uses_single_legacy_token() {
    let bytes = common::css_bundle_with_legacy_font_face();
    let bundle = decode_template(&bytes).unwrap();

    let CssFragmentBody::Tokens(body) = &bundle.css.fragments[0].body else {
        panic!("expected token CSS body");
    };
    assert_eq!(body.font_faces.len(), 1);
    assert_eq!(body.font_faces[0].family, "LegacyFace");
    assert_eq!(body.font_faces[0].tokens.len(), 1);
    assert_eq!(
        body.font_faces[0].tokens[0],
        vec![("font-family", "LegacyFace")]
    );
}

#[test]
fn config_before_css_enables_rule_body_even_when_route_lists_css_first() {
    let bytes = common::css_rule_bundle_with_config_after_css();
    let bundle = decode_template(&bytes).unwrap();

    assert!(bundle.compile_options.enable_css_rule);
    let CssFragmentBody::Rules(rules) = &bundle.css.fragments[0].body else {
        panic!("expected rule CSS body");
    };
    assert_eq!(rules.len(), 1);
    let CssRule::Style {
        position,
        selectors,
        token,
    } = &rules[0]
    else {
        panic!("expected style rule");
    };
    assert_eq!(*position, 42);
    assert!(selectors.is_empty());
    assert_eq!(token.attributes[0].0, 27);
    assert_eq!(token.attributes[0].1.value, Value::Str("12px"));
}
