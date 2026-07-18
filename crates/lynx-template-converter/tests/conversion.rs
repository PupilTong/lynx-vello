use lynx_template_converter::{ConvertError, convert};
use lynx_template_decoder::css_property::{CssPropertyId, token_types};

const SECTION_ROUTE: u8 = 10;
const SECTION_ROOT_LEPUS: u8 = 11;
const SECTION_JS_BYTECODE: u8 = 14;
const SECTION_CUSTOM: u8 = 16;

const VALUE_NIL: u8 = 0;
const VALUE_STRING: u8 = 3;
const VALUE_TABLE: u8 = 4;
const VALUE_ARRAY: u8 = 5;
const VALUE_INT32: u8 = 9;
const VALUE_UINT32: u8 = 11;
const VALUE_BYTE_ARRAY: u8 = 18;

const EMPTY_ROOT_LEPUS: &[u8] = &[
    1, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 1, 160, 1, 0, 0, 0, 194, 40, 94, 1, 12, 0, 0,
    128, 128, 128, 144, 128, 128, 128, 128, 128, 1,
];

#[test]
fn converts_source_scripts_and_css_to_a_decodable_web_bundle() {
    let native = native_bundle(vec![
        empty_root_lepus(),
        custom_section(vec![
            CustomSection::source("library", "globalThis.background = true;"),
            CustomSection::source("library__main-thread", "globalThis.mainThread = true;"),
            CustomSection::css("library:CSS", &css_fragment()),
        ]),
    ]);

    let web = convert(&native).expect("source native bundle should convert");
    let decoded = lynx_template_decoder::decode(&web).expect("output should be a web bundle");

    assert_eq!(decoded.version, 1);
    assert_eq!(decoded.config_str("cardType"), Some("react"));
    assert_eq!(decoded.config_str("isLazy"), Some("true"));
    assert_eq!(decoded.config_str("targetSdkVersion"), Some("3.5"));
    assert!(decoded.config_flag("enableCSSSelector"));
    assert_eq!(
        decoded
            .lepus_code
            .get("library__main-thread")
            .map(String::as_str),
        Some("globalThis.mainThread = true;")
    );
    assert_eq!(
        decoded.manifest.get("/library").map(String::as_str),
        Some("globalThis.background = true;")
    );
    assert_eq!(decoded.custom_sections, Some(serde_json::json!({})));

    let style_info = decoded.style_info.expect("StyleInfo section");
    let sheet = style_info
        .css_id_to_style_sheet
        .get(&7)
        .expect("native CSS id should be preserved");
    assert_eq!(sheet.rules.len(), 1);
    let rule = &sheet.rules[0];
    assert_eq!(rule.prelude.selector_list[0].to_css_string(), ".box");
    assert_eq!(rule.declaration_block.declarations.len(), 3);

    let width = &rule.declaration_block.declarations[0];
    assert_eq!(width.property_id.id, CssPropertyId::Width);
    assert_eq!(width.value_text(), "calc(100rpx - 2px)");
    assert!(width.value_token_list.iter().any(|token| {
        token.token_type == token_types::DIMENSION_TOKEN && token.value == "100rpx"
    }));

    let background = &rule.declaration_block.declarations[1];
    assert_eq!(background.property_id.id, CssPropertyId::BackgroundColor);
    assert_eq!(background.value_text(), "var(--accent, blue)");

    let variable = &rule.declaration_block.declarations[2];
    assert_eq!(variable.property_id.id, CssPropertyId::Unknown);
    assert_eq!(variable.property_id.name(), "--accent");
    assert_eq!(variable.value_text(), "#123456");
}

#[test]
fn rejects_a_bytecode_custom_section_as_code_cache() {
    let native = native_bundle_with_selector(
        vec![custom_section(vec![CustomSection::code_cache(
            "library__main-thread",
        )])],
        false,
    );

    assert!(matches!(
        convert(&native),
        Err(ConvertError::CodeCacheBundle { section })
            if section == "library__main-thread"
    ));
}

#[test]
fn rejects_a_top_level_bytecode_section_as_code_cache() {
    let native = native_bundle(vec![(SECTION_JS_BYTECODE, Vec::new())]);

    assert!(matches!(
        convert(&native),
        Err(ConvertError::CodeCacheBundle { section }) if section == "JS_BYTECODE"
    ));
}

#[test]
fn rejects_a_nonempty_root_program_as_code_cache() {
    let native = native_bundle(vec![
        root_lepus(&[1, 2, 3]),
        custom_section(vec![CustomSection::source("library", "source")]),
    ]);

    assert!(matches!(
        convert(&native),
        Err(ConvertError::CodeCacheBundle { section }) if section == "ROOT_LEPUS"
    ));
}

#[test]
fn rejects_a_mismatched_native_size() {
    let mut native = native_bundle(Vec::new());
    native[0..4].copy_from_slice(&0_u32.to_le_bytes());
    assert!(matches!(
        convert(&native),
        Err(ConvertError::SizeMismatch { .. })
    ));
}

#[derive(Debug)]
struct CustomSection {
    name: &'static str,
    encoding: i32,
    content: Vec<u8>,
}

impl CustomSection {
    fn source(name: &'static str, source: &str) -> Self {
        let mut content = Vec::new();
        lepus_string(&mut content, source);
        Self {
            name,
            encoding: 0,
            content,
        }
    }

    fn css(name: &'static str, fragment: &[u8]) -> Self {
        let mut content = vec![VALUE_BYTE_ARRAY];
        content.extend_from_slice(&(fragment.len() as u64).to_le_bytes());
        content.extend_from_slice(fragment);
        Self {
            name,
            encoding: 2,
            content,
        }
    }

    fn code_cache(name: &'static str) -> Self {
        Self {
            name,
            encoding: 1,
            content: vec![0xde, 0xad, 0xbe, 0xef],
        }
    }
}

fn native_bundle(sections: Vec<(u8, Vec<u8>)>) -> Vec<u8> {
    native_bundle_with_selector(sections, true)
}

fn native_bundle_with_selector(sections: Vec<(u8, Vec<u8>)>, enable_css_selector: bool) -> Vec<u8> {
    let mut output = vec![0; 4];
    output.extend_from_slice(&0x0024_1922_u32.to_le_bytes());
    string(&mut output, "0.2.0.0");
    string(&mut output, "test");
    string(&mut output, "3.5");
    string(&mut output, "3.5");
    header_fields(&mut output, enable_css_selector);
    output.push(VALUE_NIL);
    string(&mut output, "DynamicComponent");
    output.push(0);

    output.push(SECTION_ROUTE);
    u32_value(&mut output, sections.len());
    let mut relative_offset = 0_usize;
    for (section_type, content) in &sections {
        output.push(*section_type);
        u32_value(&mut output, relative_offset);
        relative_offset += content.len() + 1;
        u32_value(&mut output, relative_offset);
    }
    for (section_type, content) in sections {
        output.push(section_type);
        output.extend_from_slice(&content);
    }

    let size = u32::try_from(output.len()).expect("test bundle size");
    output[0..4].copy_from_slice(&size.to_le_bytes());
    output
}

fn header_fields(output: &mut Vec<u8>, enable_css_selector: bool) {
    let selector = [u8::from(enable_css_selector)];
    let fields: &[(u8, u8, &[u8])] = &[
        (0, 0, b"3.5"),
        (1, 1, &[0]),
        (1, 4, &[1]),
        (1, 6, &[1]),
        (1, 25, &[1]),
        (1, 27, &[1]),
        (1, 29, &selector),
        (1, 31, &[1]),
    ];
    let total_size = 12 + fields.iter().map(|field| 4 + field.2.len()).sum::<usize>();
    u32_value(output, total_size);
    output.extend_from_slice(&0x494e_464f_u32.to_le_bytes());
    u32_value(output, fields.len());
    for (field_type, key, payload) in fields {
        output.push(*field_type);
        output.push(*key);
        let length = u16::try_from(payload.len()).expect("field payload size");
        output.extend_from_slice(&length.to_le_bytes());
        output.extend_from_slice(payload);
    }
}

fn custom_section(sections: Vec<CustomSection>) -> (u8, Vec<u8>) {
    let mut descriptor = Vec::new();
    u32_value(&mut descriptor, sections.len());
    let mut offset = 0_usize;
    for section in &sections {
        string(&mut descriptor, section.name);
        if section.encoding == 0 {
            descriptor.push(VALUE_TABLE);
            u32_value(&mut descriptor, 0);
        } else {
            descriptor.push(VALUE_TABLE);
            u32_value(&mut descriptor, 1);
            string(&mut descriptor, "encoding");
            descriptor.push(VALUE_INT32);
            descriptor.extend_from_slice(&section.encoding.to_le_bytes());
        }
        u32_value(&mut descriptor, offset);
        offset += section.content.len();
        u32_value(&mut descriptor, offset);
    }
    for section in sections {
        descriptor.extend_from_slice(&section.content);
    }
    (SECTION_CUSTOM, descriptor)
}

fn empty_root_lepus() -> (u8, Vec<u8>) {
    root_lepus(EMPTY_ROOT_LEPUS)
}

fn root_lepus(bytecode: &[u8]) -> (u8, Vec<u8>) {
    let mut content = Vec::new();
    content.extend_from_slice(&(bytecode.len() as u64).to_le_bytes());
    content.extend_from_slice(bytecode);
    (SECTION_ROOT_LEPUS, content)
}

fn css_fragment() -> Vec<u8> {
    let mut output = Vec::new();
    u32_value(&mut output, 7);
    u32_value(&mut output, 0);
    u32_value(&mut output, 1);
    u32_value(&mut output, 1);

    output.push(VALUE_ARRAY);
    u32_value(&mut output, 3);
    output.push(VALUE_UINT32);
    output.extend_from_slice(&((3_u32 << 4) | (1 << 16) | (1 << 17)).to_le_bytes());
    output.push(VALUE_UINT32);
    output.extend_from_slice(&0_u32.to_le_bytes());
    lepus_string(&mut output, "box");

    u32_value(&mut output, 2);
    css_declaration(
        &mut output,
        CssPropertyId::Width,
        "calc(100rpx - 2px)",
        None,
    );
    css_declaration(
        &mut output,
        CssPropertyId::BackgroundColor,
        "{{--accent}}",
        Some(("--accent", "blue")),
    );
    u32_value(&mut output, 1);
    string(&mut output, "--accent");
    string(&mut output, "#123456");
    u32_value(&mut output, 0);
    output
}

fn css_declaration(
    output: &mut Vec<u8>,
    property: CssPropertyId,
    value: &str,
    fallback: Option<(&str, &str)>,
) {
    output.extend_from_slice(&(property as u32).to_le_bytes());
    lepus_string(output, value);
    output.extend_from_slice(&0_u32.to_le_bytes());
    string(output, "");
    match fallback {
        Some((name, value)) => {
            output.push(VALUE_TABLE);
            u32_value(output, 1);
            string(output, name);
            lepus_string(output, value);
        }
        None => output.push(VALUE_NIL),
    }
}

fn lepus_string(output: &mut Vec<u8>, value: &str) {
    output.push(VALUE_STRING);
    string(output, value);
}

fn string(output: &mut Vec<u8>, value: &str) {
    u32_value(output, value.len());
    output.extend_from_slice(value.as_bytes());
}

fn u32_value(output: &mut Vec<u8>, value: usize) {
    output.extend_from_slice(
        &u32::try_from(value)
            .expect("test value should fit in u32")
            .to_le_bytes(),
    );
}
