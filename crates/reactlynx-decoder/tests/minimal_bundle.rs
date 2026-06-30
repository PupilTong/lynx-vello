mod common;

use reactlynx_decoder::{DecodeError, Version, decode_template};

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
