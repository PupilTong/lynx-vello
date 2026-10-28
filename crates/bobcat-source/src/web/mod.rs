//! Decoder for the Lynx **web** binary template (`.web.bundle`).

mod config;
pub mod css_property;
pub mod error;
mod reader;
mod sections;
pub mod style_info;

use std::collections::BTreeMap;

use bobcat_core::PageConfig;
pub use error::DecodeError;
use reader::Reader;
pub use style_info::{StyleInfo, StyleInfoMap};

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
    /// The custom sections the compiler tagged `encoding: 'CSS'`, as an rkyv
    /// [`StyleInfoMap`] keyed by section name (lynx-stack#4138,
    /// `TemplateSectionLabel.CustomSectionsStyleInfo` in web-core's
    /// `ts/constants.ts`). The encoder writes it only when such a section
    /// exists, and those sections are then absent from `CustomSections`.
    CustomSectionsStyleInfo = 7,
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
    /// The `CustomSectionsStyleInfo` section: one [`StyleInfo`] per named
    /// CSS custom section, empty when the section is absent. The native
    /// decoder fills it from a `.lynx.bundle`'s CSS custom sections.
    pub custom_sections_style_info: BTreeMap<String, StyleInfo>,
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
    let sections = Sections::<serde_json::Map<String, serde_json::Value>>::decode(bytes)?;
    let owned = |map: &BTreeMap<&str, &str>| -> BTreeMap<String, String> {
        map.iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    };
    let style_info = sections
        .style_info
        .map(style_info::decode_style_info)
        .transpose()?;
    let custom_sections_style_info = match sections.custom_sections_style_info {
        Some(bytes) => style_info::decode_style_info_map(bytes)?
            .name_to_style_info
            .into_iter()
            .collect(),
        None => BTreeMap::new(),
    };
    Ok(WebTemplate {
        version: sections.version,
        config: sections.config.unwrap_or_default(),
        lepus_code: owned(&sections.lepus_code),
        manifest: owned(&sections.manifest),
        custom_sections: sections.custom_sections,
        style_info,
        custom_sections_style_info,
        element_templates: sections.element_templates.map(<[u8]>::to_vec),
    })
}

/// A web container's sections, borrowed from its bytes wherever they can be:
/// a registration copies a body once, into the text it registers, and lowers
/// a `StyleInfo` from where it lies.
///
/// `C` is what the `Configurations` object is read as: the whole object for
/// [`decode`], only the [`PageConfig`] a registration needs.
///
/// Decoding one validates exactly what [`decode`] validates, in the same
/// order, except the two rkyv sections, which are left to whoever reads them
/// — the full decode checks them right after, `StyleInfo` first.
#[derive(Debug)]
#[expect(
    clippy::struct_field_names,
    reason = "one field per section, named as `WebTemplate` names it"
)]
pub(crate) struct Sections<'a, C> {
    pub(crate) version: u32,
    pub(crate) config: Option<C>,
    pub(crate) lepus_code: BTreeMap<&'a str, &'a str>,
    pub(crate) manifest: BTreeMap<&'a str, &'a str>,
    pub(crate) custom_sections: Option<serde_json::Value>,
    pub(crate) style_info: Option<&'a [u8]>,
    pub(crate) custom_sections_style_info: Option<&'a [u8]>,
    pub(crate) element_templates: Option<&'a [u8]>,
}

/// What a `Configurations` object is read as.
pub(crate) trait ReadConfig: Sized {
    /// `Ok(None)` for JSON that is no object.
    fn read(text: &str) -> Result<Option<Self>, serde_json::Error>;
}

impl ReadConfig for serde_json::Map<String, serde_json::Value> {
    fn read(text: &str) -> Result<Option<Self>, serde_json::Error> {
        Ok(match serde_json::from_str(text)? {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
    }
}

impl ReadConfig for PageConfig {
    fn read(text: &str) -> Result<Option<Self>, serde_json::Error> {
        config::page_config(text)
    }
}

impl<'a, C: ReadConfig> Sections<'a, C> {
    pub(crate) fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
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

        let mut sections = Self {
            version,
            config: None,
            lepus_code: BTreeMap::new(),
            manifest: BTreeMap::new(),
            custom_sections: None,
            style_info: None,
            custom_sections_style_info: None,
            element_templates: None,
        };
        while !reader.is_empty() {
            let label = reader.read_u32()?;
            let length = reader.read_u32()? as usize;
            let content = reader.read_bytes(length)?;

            match label {
                l if l == SectionLabel::Configurations as u32 => {
                    const SECTION: &str = "Configurations";
                    let text = sections::decode_utf16_text(content, SECTION)?;
                    let config = C::read(&text)
                        .map_err(|source| DecodeError::Json {
                            section: SECTION,
                            source,
                        })?
                        .ok_or_else(|| DecodeError::InvalidSection {
                            section: SECTION,
                            reason: "expected a JSON object".to_owned(),
                        })?;
                    sections.config = Some(config);
                }
                l if l == SectionLabel::LepusCode as u32 => {
                    sections.lepus_code = sections::decode_string_map(content, "LepusCode")?;
                }
                l if l == SectionLabel::Manifest as u32 => {
                    sections.manifest = sections::decode_string_map(content, "Manifest")?;
                }
                l if l == SectionLabel::CustomSections as u32 => {
                    sections.custom_sections =
                        Some(sections::decode_utf16_json(content, "CustomSections")?);
                }
                l if l == SectionLabel::StyleInfo as u32 => {
                    sections.style_info = Some(content);
                }
                l if l == SectionLabel::CustomSectionsStyleInfo as u32 => {
                    sections.custom_sections_style_info = Some(content);
                }
                l if l == SectionLabel::ElementTemplates as u32 => {
                    sections.element_templates = Some(content);
                }
                other => return Err(DecodeError::UnknownSection(other)),
            }
        }
        Ok(sections)
    }
}

impl Sections<'_, PageConfig> {
    /// The page flags the `Configurations` section sets, every one off when
    /// it is absent.
    pub(crate) fn page_config(&self) -> PageConfig {
        self.config.unwrap_or_else(config::all_off)
    }
}

/// What a view starts from, read from a web container without decoding its
/// sections: the page flags, and which of the sections a page's view names
/// it carries.
#[derive(Debug)]
pub(crate) struct Scan {
    pub(crate) config: PageConfig,
    /// Whether the (last) `LepusCode` section names a `root` chunk.
    pub(crate) has_root: bool,
    pub(crate) has_style_info: bool,
}

/// Reads the container header, every section's label and length and the
/// `Configurations` section, and the `LepusCode` section's keys only. It
/// fails as [`decode`] fails on the header, an unknown or truncated section,
/// and the `Configurations` section; anything inside another section is
/// left to the decode.
pub(crate) fn scan(bytes: &[u8]) -> Result<Scan, DecodeError> {
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

    let mut scan = Scan {
        config: config::all_off(),
        has_root: false,
        has_style_info: false,
    };
    while !reader.is_empty() {
        let label = reader.read_u32()?;
        let length = reader.read_u32()? as usize;
        let content = reader.read_bytes(length)?;
        match label {
            l if l == SectionLabel::Configurations as u32 => {
                const SECTION: &str = "Configurations";
                let text = sections::decode_utf16_text(content, SECTION)?;
                scan.config = PageConfig::read(&text)
                    .map_err(|source| DecodeError::Json {
                        section: SECTION,
                        source,
                    })?
                    .ok_or_else(|| DecodeError::InvalidSection {
                        section: SECTION,
                        reason: "expected a JSON object".to_owned(),
                    })?;
            }
            l if l == SectionLabel::LepusCode as u32 => {
                scan.has_root = sections::string_map_has_key(content, b"root");
            }
            l if l == SectionLabel::StyleInfo as u32 => scan.has_style_info = true,
            l if l == SectionLabel::Manifest as u32
                || l == SectionLabel::CustomSections as u32
                || l == SectionLabel::CustomSectionsStyleInfo as u32
                || l == SectionLabel::ElementTemplates as u32 => {}
            other => return Err(DecodeError::UnknownSection(other)),
        }
    }
    Ok(scan)
}
