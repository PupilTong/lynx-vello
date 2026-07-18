use std::collections::{BTreeMap, HashMap};

use lynx_template_decoder::style_info::{StyleInfo, StyleSheet};
use lynx_template_decoder::{MAGIC_0, MAGIC_1, SectionLabel};
use serde_json::{Map, Value as JsonValue};

use crate::ConvertError;
use crate::native::NativeBundle;

const WEB_FORMAT_VERSION: u32 = 1;

pub(crate) fn encode(native: NativeBundle) -> Result<Vec<u8>, ConvertError> {
    let config = encode_utf16_json(&JsonValue::Object(native.config), "Configurations")?;
    let lepus_code = encode_string_map(&native.lepus_code, "LepusCode")?;
    let custom_sections = encode_utf16_json(&JsonValue::Object(Map::default()), "CustomSections")?;
    let style_info = encode_style_info(native.styles)?;
    let manifest = encode_string_map(&native.manifest, "Manifest")?;

    let mut output = Vec::new();
    output.extend_from_slice(&MAGIC_0.to_le_bytes());
    output.extend_from_slice(&MAGIC_1.to_le_bytes());
    output.extend_from_slice(&WEB_FORMAT_VERSION.to_le_bytes());
    append_section(
        &mut output,
        SectionLabel::Configurations,
        &config,
        "Configurations",
    )?;
    append_section(
        &mut output,
        SectionLabel::LepusCode,
        &lepus_code,
        "LepusCode",
    )?;
    append_section(
        &mut output,
        SectionLabel::CustomSections,
        &custom_sections,
        "CustomSections",
    )?;
    append_section(
        &mut output,
        SectionLabel::StyleInfo,
        &style_info,
        "StyleInfo",
    )?;
    append_section(&mut output, SectionLabel::Manifest, &manifest, "Manifest")?;
    Ok(output)
}

fn encode_style_info(styles: BTreeMap<i32, StyleSheet>) -> Result<Vec<u8>, ConvertError> {
    let style_info = StyleInfo {
        css_id_to_style_sheet: styles.into_iter().collect::<HashMap<_, _>>(),
        style_content_str_size_hint: 0,
    };
    rkyv::to_bytes::<_, 1024>(&style_info)
        .map(|bytes| bytes.to_vec())
        .map_err(|error| ConvertError::StyleInfo(error.to_string()))
}

fn encode_string_map(
    values: &BTreeMap<String, String>,
    section: &'static str,
) -> Result<Vec<u8>, ConvertError> {
    let mut output = Vec::new();
    write_len(&mut output, values.len(), section)?;
    for (key, value) in values {
        write_bytes(&mut output, key.as_bytes(), section)?;
        write_bytes(&mut output, value.as_bytes(), section)?;
    }
    Ok(output)
}

fn encode_utf16_json(value: &JsonValue, section: &'static str) -> Result<Vec<u8>, ConvertError> {
    let source = serde_json::to_string(value)
        .map_err(|source| ConvertError::OutputJson { section, source })?;
    let mut output = Vec::with_capacity(source.len().saturating_mul(2));
    for code_unit in source.encode_utf16() {
        output.extend_from_slice(&code_unit.to_le_bytes());
    }
    Ok(output)
}

fn append_section(
    output: &mut Vec<u8>,
    label: SectionLabel,
    payload: &[u8],
    section: &'static str,
) -> Result<(), ConvertError> {
    output.extend_from_slice(&(label as u32).to_le_bytes());
    write_len(output, payload.len(), section)?;
    output.extend_from_slice(payload);
    Ok(())
}

fn write_bytes(
    output: &mut Vec<u8>,
    value: &[u8],
    section: &'static str,
) -> Result<(), ConvertError> {
    write_len(output, value.len(), section)?;
    output.extend_from_slice(value);
    Ok(())
}

fn write_len(
    output: &mut Vec<u8>,
    length: usize,
    section: &'static str,
) -> Result<(), ConvertError> {
    let length =
        u32::try_from(length).map_err(|_| ConvertError::OutputTooLarge { section, length })?;
    output.extend_from_slice(&length.to_le_bytes());
    Ok(())
}
