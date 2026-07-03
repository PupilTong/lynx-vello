//! Decoder for the Lynx **web** binary template (`.web.bundle`).
//!
//! This is the bundle format produced for the Lynx *web* target by
//! `@lynx-js/web-core/encode` (the `SDRA WROF` container) and consumed at
//! runtime by `decodeTemplate()` in `@lynx-js/web-core`. This crate is a
//! native Rust port of that decoder.
//!
//! Scope: binary template parsing only — no Lepus VM, no CSS engine, no JS
//! runtime. The JSON (non-binary) web bundle variant is intentionally not
//! supported.
//!
//! See `docs/web-binary-template.md` in this repository for the full format
//! specification, and `docs/lynx-binary-template.md` for the very different
//! native `.lynx.bundle` format.
//!
//! # Example
//!
//! ```no_run
//! let bytes = std::fs::read("main.web.bundle").unwrap();
//! let template = lynx_template_decoder::decode(&bytes).unwrap();
//! println!("card type: {:?}", template.config.get("cardType"));
//! println!(
//!     "main-thread JS: {} bytes",
//!     template.lepus_code["root"].len()
//! );
//! ```

pub mod css_property;
pub mod error;
mod reader;
mod sections;
pub mod style_info;

use std::collections::BTreeMap;

pub use error::DecodeError;
use reader::Reader;
pub use style_info::StyleInfo;

/// First magic word of a web bundle: `"SDRA"` as a little-endian u32.
pub const MAGIC_0: u32 = 0x4152_4453;
/// Second magic word of a web bundle: `"WROF"` as a little-endian u32.
pub const MAGIC_1: u32 = 0x464F_5257;

/// Section labels of the web bundle container
/// (`TemplateSectionLabel` in web-core).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SectionLabel {
    /// Background-thread JS chunks: path → source.
    Manifest = 1,
    /// rkyv-serialized [`StyleInfo`].
    StyleInfo = 2,
    /// Main-thread JS: chunk name (`root`, …) → source.
    LepusCode = 3,
    /// UTF-16LE JSON: `Record<string, { type?: 'lazy', content: … }>`.
    CustomSections = 4,
    /// Reserved; never emitted by the current encoder.
    ElementTemplates = 5,
    /// UTF-16LE JSON string map: `cardType`, `isLazy`, page config flags.
    Configurations = 6,
}

/// A decoded web binary template.
///
/// Mirrors (and extends) the `DecodedTemplate` shape of the reference
/// TypeScript decoder: this decoder also surfaces `manifest` and keeps the
/// raw bytes of the reserved `ElementTemplates` section.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct WebTemplate {
    /// Container format version (currently always `1`).
    pub version: u32,
    /// The `Configurations` section: `cardType`, `isLazy`, and stringified
    /// page config flags (`enableCSSSelector`, `defaultDisplayLinear`, …).
    pub config: serde_json::Map<String, serde_json::Value>,
    /// The `LepusCode` section: main-thread JavaScript source by chunk name.
    /// The entry chunk is `"root"`.
    pub lepus_code: BTreeMap<String, String>,
    /// The `Manifest` section: background-thread JavaScript source by path
    /// (e.g. `"/app-service.js"`).
    pub manifest: BTreeMap<String, String>,
    /// The `CustomSections` section, if present.
    pub custom_sections: Option<serde_json::Value>,
    /// The `StyleInfo` section (pre-parsed CSS), if present.
    pub style_info: Option<StyleInfo>,
    /// Raw bytes of the reserved `ElementTemplates` section, if present.
    pub element_templates: Option<Vec<u8>>,
}

impl WebTemplate {
    /// Convenience accessor for string values in [`Self::config`].
    #[must_use]
    pub fn config_str(&self, key: &str) -> Option<&str> {
        self.config.get(key).and_then(serde_json::Value::as_str)
    }

    /// Whether a boolean-ish config flag (stored as `"true"` / `"false"`)
    /// is set.
    #[must_use]
    pub fn config_flag(&self, key: &str) -> bool {
        self.config_str(key) == Some("true")
    }
}

/// Decodes a Lynx web binary template bundle.
///
/// This is the native equivalent of `decodeTemplate()` in
/// `@lynx-js/web-core` — with the difference that the `StyleInfo` section is
/// returned as structured data ([`StyleInfo`]) instead of being flattened to
/// CSS text, and that trailing garbage after the last section is an error
/// rather than being silently ignored.
pub fn decode(bytes: &[u8]) -> Result<WebTemplate, DecodeError> {
    let mut reader = Reader::new(bytes);

    let magic0 = reader.u32()?;
    let magic1 = reader.u32()?;
    if magic0 != MAGIC_0 || magic1 != MAGIC_1 {
        return Err(DecodeError::BadMagic { magic0, magic1 });
    }

    let version = reader.u32()?;
    if version > 1 {
        return Err(DecodeError::UnsupportedVersion(version));
    }

    let mut template = WebTemplate {
        version,
        ..WebTemplate::default()
    };
    let mut style_info_bytes: Option<&[u8]> = None;

    while !reader.is_empty() {
        let label = reader.u32()?;
        let length = reader.u32()? as usize;
        let content = reader.take(length)?;

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
                // Deferred: decoding needs no config, but keeping the raw
                // slice avoids a copy if a later duplicate section wins.
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
