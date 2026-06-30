#![allow(dead_code)]

pub(crate) const QUICK_MAGIC: u32 = 0x0024_1922;
const HEADER_EXT_MAGIC: u32 = 0x494e_464f;
const SECTION_ROUTE: u8 = 10;
const SECTION_CONFIG: u8 = 6;
const SECTION_CSS: u8 = 1;
const SECTION_NEW_ELEMENT_TEMPLATE: u8 = 17;

pub(crate) fn minimal_config_bundle(json: &str) -> Vec<u8> {
    let mut config = vec![SECTION_CONFIG];
    push_lstr(&mut config, json);
    bundle_with_sections(vec![config])
}

pub(crate) fn minimal_element_template_bundle() -> Vec<u8> {
    let mut section = vec![SECTION_NEW_ELEMENT_TEMPLATE];
    push_element_template_section_body(&mut section);
    bundle_with_sections(vec![section])
}

pub(crate) fn minimal_css_bundle() -> Vec<u8> {
    let mut section = vec![SECTION_CSS];
    section.extend_from_slice(&0u32.to_le_bytes());
    bundle_with_sections(vec![section])
}

fn bundle_with_sections(sections: Vec<Vec<u8>>) -> Vec<u8> {
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

    out.push(SECTION_ROUTE);
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    let mut offset = 0u32;
    for section in &sections {
        out.push(section[0]);
        out.extend_from_slice(&offset.to_le_bytes());
        offset += section.len() as u32;
        out.extend_from_slice(&offset.to_le_bytes());
    }
    for section in sections {
        out.extend_from_slice(&section);
    }

    let total_size = out.len() as u32;
    out[0..4].copy_from_slice(&total_size.to_le_bytes());
    out
}

fn push_element_template_section_body(out: &mut Vec<u8>) {
    out.extend_from_slice(&1u32.to_le_bytes()); // router count
    push_lstr(out, "_et_test");
    out.extend_from_slice(&0u32.to_le_bytes()); // first body starts at descriptor offset
    out.extend_from_slice(&1u32.to_le_bytes()); // root count
    push_element(
        out,
        0,
        |out| {
            out.push(6); // ELEMENT_CLASS
            out.extend_from_slice(&1u32.to_le_bytes());
            push_lstr(out, "container");
        },
        |out| {
            push_element(
                out,
                1,
                |_| {},
                |out| {
                    push_element(
                        out,
                        2,
                        |out| {
                            out.push(14); // ELEMENT_ATTRIBUTE_ARRAY
                            out.extend_from_slice(&1u32.to_le_bytes());
                            out.extend_from_slice(&0u32.to_le_bytes()); // static
                            push_lstr(out, "text");
                            push_value_str(out, "hello");
                        },
                        |_| {},
                    );
                },
            );
        },
    );
}

fn push_element(
    out: &mut Vec<u8>,
    tag: u8,
    sections: impl FnOnce(&mut Vec<u8>),
    children: impl FnOnce(&mut Vec<u8>),
) {
    let offset_pos = out.len();
    out.extend_from_slice(&0u32.to_le_bytes());
    let field_end = out.len();
    out.push(1); // ELEMENT_TAG_ENUM
    out.push(tag);
    sections(out);
    let children_offset = (out.len() - field_end) as u32;
    out[offset_pos..offset_pos + 4].copy_from_slice(&children_offset.to_le_bytes());
    out.push(5); // ELEMENT_CHILDREN
    let count_pos = out.len();
    out.extend_from_slice(&0u32.to_le_bytes());
    let before_children = out.len();
    children(out);
    let child_count = count_direct_children(&out[before_children..]);
    out[count_pos..count_pos + 4].copy_from_slice(&child_count.to_le_bytes());
}

fn count_direct_children(bytes: &[u8]) -> u32 {
    u32::from(!bytes.is_empty())
}

fn push_value_str(out: &mut Vec<u8>, value: &str) {
    out.push(3);
    push_lstr(out, value);
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
