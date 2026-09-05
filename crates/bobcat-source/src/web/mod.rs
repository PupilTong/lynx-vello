//! Decoder for the Lynx **web** binary template (`.web.bundle`).

pub mod css_property;
pub mod error;
mod reader;
mod sections;
pub mod style_info;

use std::collections::BTreeMap;

pub use error::DecodeError;
use reader::Reader;
pub use style_info::StyleInfo;

pub const MAGIC_0: u32 = 0x4152_4453;
pub const MAGIC_1: u32 = 0x464F_5257;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SectionLabel {
    Manifest = 1,
    StyleInfo = 2,
    LepusCode = 3,
    CustomSections = 4,
    ElementTemplates = 5,
    Configurations = 6,
}

/// A decoded web binary template.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct WebTemplate {
    pub version: u32,
    pub config: serde_json::Map<String, serde_json::Value>,
    pub lepus_code: BTreeMap<String, String>,
    pub manifest: BTreeMap<String, String>,
    pub custom_sections: Option<serde_json::Value>,
    pub style_info: Option<StyleInfo>,
    pub element_templates: Option<Vec<u8>>,
}

impl WebTemplate {
    #[must_use]
    pub fn config_str(&self, key: &str) -> Option<&str> {
        self.config.get(key).and_then(serde_json::Value::as_str)
    }

    #[must_use]
    pub fn config_flag(&self, key: &str) -> bool {
        self.config_str(key) == Some("true")
    }
}

pub fn decode(bytes: &[u8]) -> Result<WebTemplate, DecodeError> {
    let mut reader = Reader::new(bytes);

    let magic0 = reader.read_u32()?;
    let magic1 = reader.read_u32()?;
    if magic0 != MAGIC_0 || magic1 != MAGIC_1 {
        return Err(DecodeError::BadMagic { magic0, magic1 });
    }

    let version = reader.read_u32()?;
    if version > 1 {
        return Err(DecodeError::UnsupportedVersion(version));
    }

    let mut template = WebTemplate {
        version,
        ..WebTemplate::default()
    };
    let mut style_info_bytes: Option<&[u8]> = None;

    while !reader.is_empty() {
        let label = reader.read_u32()?;
        let length = reader.read_u32()? as usize;
        let content = reader.read_bytes(length)?;

        match label {
            l if l == SectionLabel::Configurations as u32 => {
                let value = sections::decode_utf16_json(content, "Configurations")?;
                let serde_json::Value::Object(map) = value else {
                    return Err(DecodeError::InvalidSection {
                        section: "Configurations",
                        reason: "expected a JSON object".to_owned(),
                    });
                };
                template.config = map;
            }
            l if l == SectionLabel::LepusCode as u32 => {
                template.lepus_code = sections::decode_string_map(content, "LepusCode")?;
            }
            l if l == SectionLabel::Manifest as u32 => {
                template.manifest = sections::decode_string_map(content, "Manifest")?;
            }
            l if l == SectionLabel::CustomSections as u32 => {
                template.custom_sections =
                    Some(sections::decode_utf16_json(content, "CustomSections")?);
            }
            l if l == SectionLabel::StyleInfo as u32 => {
                style_info_bytes = Some(content);
            }
            l if l == SectionLabel::ElementTemplates as u32 => {
                template.element_templates = Some(content.to_vec());
            }
            other => return Err(DecodeError::UnknownSection(other)),
        }
    }

    if let Some(bytes) = style_info_bytes {
        template.style_info = Some(style_info::decode_style_info(bytes)?);
    }

    Ok(template)
}
