#![allow(dead_code)]

pub(crate) const QUICK_MAGIC: u32 = 0x0024_1922;
const HEADER_EXT_MAGIC: u32 = 0x494e_464f;
const SECTION_ROUTE: u8 = 10;
const SECTION_CONFIG: u8 = 6;

pub(crate) fn minimal_config_bundle(json: &str) -> Vec<u8> {
    let target_sdk = "3.9.0";
    let mut out = Vec::new();
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&QUICK_MAGIC.to_le_bytes());
    push_lstr(&mut out, "1.0.0.0");
    push_lstr(&mut out, "");
    push_lstr(&mut out, target_sdk);
    push_lstr(&mut out, target_sdk);
    push_header_ext(&mut out, target_sdk);
    out.push(0); // template_info = Nil
    push_lstr(&mut out, "card");
    out.push(0); // snapshot

    let mut config = vec![SECTION_CONFIG];
    push_lstr(&mut config, json);

    out.push(SECTION_ROUTE);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(SECTION_CONFIG);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(config.len() as u32).to_le_bytes());
    out.extend_from_slice(&config);

    let total_size = out.len() as u32;
    out[0..4].copy_from_slice(&total_size.to_le_bytes());
    out
}

fn push_header_ext(out: &mut Vec<u8>, target_sdk: &str) {
    let mut fields = Vec::new();
    push_field(&mut fields, 0, 0, target_sdk.as_bytes());
    push_field(&mut fields, 1, 1, &[1]); // enable_css_parser
    push_field(&mut fields, 1, 6, &[1]); // enable_css_variable
    push_field(&mut fields, 1, 25, &[1]); // enable_fiber_arch
    push_field(&mut fields, 1, 27, &[1]); // enable_flexible_template
    push_field(&mut fields, 1, 28, &[1]); // arch_option = FIBER_ARCH
    push_field(&mut fields, 1, 29, &[1]); // enable_css_selector
    push_field(&mut fields, 1, 33, &[1]); // enable_simple_styling

    let field_count = 8u32;
    let size = 12u32 + fields.len() as u32;
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&HEADER_EXT_MAGIC.to_le_bytes());
    out.extend_from_slice(&field_count.to_le_bytes());
    out.extend_from_slice(&fields);
}

fn push_field(out: &mut Vec<u8>, field_type: u8, key_id: u8, payload: &[u8]) {
    out.push(field_type);
    out.push(key_id);
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(payload);
}

fn push_lstr(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}
