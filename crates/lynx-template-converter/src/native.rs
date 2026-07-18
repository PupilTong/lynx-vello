use std::collections::{BTreeMap, HashMap};

use lynx_template_decoder::style_info::StyleSheet;
use serde_json::{Map, Value as JsonValue};

use crate::ConvertError;
use crate::reader::Reader;
use crate::style::{CssOptions, decode_fragment};

const QUICKJS_MAGIC: u32 = 0x0024_1922;
const HEADER_INFO_MAGIC: u32 = 0x494e_464f;

const SECTION_CSS: u8 = 1;
const SECTION_JS: u8 = 5;
const SECTION_CONFIG: u8 = 6;
const SECTION_ROUTE: u8 = 10;
const SECTION_ROOT_LEPUS: u8 = 11;
const SECTION_JS_BYTECODE: u8 = 14;
const SECTION_LEPUS_CHUNK: u8 = 15;
const SECTION_CUSTOM: u8 = 16;

const CUSTOM_SOURCE: i32 = 0;
const CUSTOM_JS_BYTECODE: i32 = 1;
const CUSTOM_CSS: i32 = 2;

const VALUE_RECURSION_LIMIT: usize = 128;

// QuickJS bytecode emitted by the current native encoder for an empty MTS
// program. Source-based external bundles still carry this inert root section;
// accepting only the exact empty program avoids silently discarding real MTS
// code that cannot be decompiled.
const EMPTY_ROOT_LEPUS_INLINE_DEBUG: &[u8] = &[
    1, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 1, 160, 1, 0, 0, 0, 194, 40, 94, 1, 12, 0, 0,
    128, 128, 128, 144, 128, 128, 128, 128, 128, 1,
];
const EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG: &[u8] = &[
    9, 204, 1, 176, 202, 1, 0, 0, 0, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 1, 160, 1, 0, 0,
    0, 194, 40, 94, 1, 0,
];

#[derive(Debug)]
pub(crate) struct NativeBundle {
    pub(crate) config: Map<String, JsonValue>,
    pub(crate) lepus_code: BTreeMap<String, String>,
    pub(crate) manifest: BTreeMap<String, String>,
    pub(crate) styles: BTreeMap<i32, StyleSheet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    major: u32,
    minor: u32,
}

impl Version {
    const V1_6: Self = Self::new(1, 6);
    const V2_0: Self = Self::new(2, 0);
    const V2_7: Self = Self::new(2, 7);
    const V2_8: Self = Self::new(2, 8);
    const V2_14: Self = Self::new(2, 14);
    const V3_9: Self = Self::new(3, 9);

    const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    fn parse(source: &str) -> Result<Self, ConvertError> {
        let mut components = source.split(['.', '-']);
        let major = components
            .next()
            .and_then(|component| component.parse().ok())
            .ok_or_else(|| ConvertError::UnsupportedVersion(source.to_owned()))?;
        let minor = components
            .next()
            .and_then(|component| component.parse().ok())
            .ok_or_else(|| ConvertError::UnsupportedVersion(source.to_owned()))?;
        Ok(Self { major, minor })
    }

    pub(crate) const fn supports_css_variables(self) -> bool {
        self.major >= Self::V2_0.major
    }

    pub(crate) const fn supports_variable_default_map(self) -> bool {
        self.major > Self::V2_14.major
            || (self.major == Self::V2_14.major && self.minor >= Self::V2_14.minor)
    }

    pub(crate) const fn supports_important(self) -> bool {
        self.major > Self::V3_9.major
            || (self.major == Self::V3_9.major && self.minor >= Self::V3_9.minor)
    }

    pub(crate) const fn supports_extended_font_face(self) -> bool {
        self.major > Self::V2_7.major
            || (self.major == Self::V2_7.major && self.minor >= Self::V2_7.minor)
    }
}

#[derive(Debug, Clone)]
struct HeaderField {
    field_type: u8,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct HeaderFields(BTreeMap<u8, HeaderField>);

impl HeaderFields {
    fn bool(&self, key: u8) -> Option<bool> {
        let field = self.0.get(&key)?;
        (field.field_type == 1 && field.payload.len() == 1).then(|| field.payload[0] != 0)
    }

    fn i32(&self, key: u8) -> Option<i32> {
        let field = self.0.get(&key)?;
        if field.field_type != 7 {
            return None;
        }
        let payload: [u8; 4] = field.payload.as_slice().try_into().ok()?;
        Some(i32::from_le_bytes(payload))
    }

    fn string(&self, key: u8) -> Option<&str> {
        let field = self.0.get(&key)?;
        (field.field_type == 0)
            .then(|| std::str::from_utf8(&field.payload).ok())
            .flatten()
    }
}

#[derive(Debug, Clone, Copy)]
struct SectionRange {
    start: usize,
    end: usize,
}

#[derive(Debug)]
struct CustomHeader {
    name: String,
    encoding: i32,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
pub(crate) enum LepusValue {
    Nil,
    Number(f64),
    Bool(bool),
    String(String),
    Table(BTreeMap<String, Self>),
    Array(Vec<Self>),
    ByteArray(Vec<u8>),
    Undefined,
}

impl LepusValue {
    pub(crate) fn number_i32(&self) -> Option<i32> {
        match *self {
            Self::Number(value)
                if value.is_finite()
                    && value.fract() == 0.0
                    && value >= f64::from(i32::MIN)
                    && value <= f64::from(i32::MAX) =>
            {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the integer range and fractional part are checked above"
                )]
                Some(value as i32)
            }
            _ => None,
        }
    }

    pub(crate) fn string_value(&self) -> Option<&str> {
        let Self::String(value) = self else {
            return None;
        };
        Some(value)
    }

    pub(crate) fn array(&self) -> Option<&[Self]> {
        let Self::Array(value) = self else {
            return None;
        };
        Some(value)
    }

    pub(crate) fn table(&self) -> Option<&BTreeMap<String, Self>> {
        let Self::Table(value) = self else {
            return None;
        };
        Some(value)
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<NativeBundle, ConvertError> {
    let mut reader = Reader::new(bytes);
    let declared_size = reader.u32()?;
    if usize::try_from(declared_size).ok() != Some(bytes.len()) {
        return Err(ConvertError::SizeMismatch {
            declared: declared_size,
            actual: bytes.len(),
        });
    }

    let magic = reader.u32()?;
    if magic != QUICKJS_MAGIC {
        return Err(ConvertError::BadNativeMagic(magic));
    }

    let _lepus_version = reader.string("Lepus version")?;
    let _cli_version = reader.string("CLI version")?;
    let ios_version = reader.string("iOS target version")?;
    let _android_version = reader.string("Android target version")?;
    let mut target_version_source = ios_version;
    let mut target_version = Version::parse(&target_version_source)?;
    if target_version < Version::V1_6 {
        return Err(ConvertError::UnsupportedVersion(target_version_source));
    }

    let fields = decode_header_fields(&mut reader)?;
    if let Some(header_version) = fields.string(0) {
        header_version.clone_into(&mut target_version_source);
        target_version = Version::parse(header_version)?;
    }
    if target_version < Version::V2_8 {
        return Err(ConvertError::UnsupportedVersion(target_version_source));
    }

    if target_version >= Version::V2_7 {
        let _template_info = decode_value(&mut reader, 0)?;
    }
    if fields.bool(20).unwrap_or(false) {
        let _trial_options = decode_value(&mut reader, 0)?;
    }

    let app_type = reader.string("app type")?;
    let _snapshot = reader.u8()?;
    if !fields.bool(27).unwrap_or(false) {
        return Err(ConvertError::UnsupportedBundle(
            "only flexible (section-routed) native bundles can be converted".to_owned(),
        ));
    }

    let routes = decode_section_routes(&mut reader, bytes.len())?;
    reject_top_level_bytecode(bytes, &routes, &app_type)?;
    if let Some(range) = routes.get(&SECTION_CUSTOM) {
        reject_custom_bytecode(bytes, *range)?;
    }

    if let Some(range) = routes.get(&SECTION_CSS)
        && !is_empty_counted_section(bytes, *range, SECTION_CSS, "CSS")?
    {
        return Err(ConvertError::UnsupportedBundle(
            "card-style top-level CSS sections are not source external bundles".to_owned(),
        ));
    }

    let page_config = routes
        .get(&SECTION_CONFIG)
        .map(|range| decode_page_config(bytes, *range))
        .transpose()?
        .unwrap_or_default();
    if config_flag(&page_config, "enableCSSRule") {
        return Err(ConvertError::UnsupportedCss(
            "the native CSS-rule encoding is not reversible to web StyleInfo".to_owned(),
        ));
    }

    let css_options = CssOptions {
        target_version,
        enable_css_parser: fields.bool(1).unwrap_or(false),
        enable_css_variable: fields.bool(6).unwrap_or(false),
        enable_css_selector: fields.bool(29).unwrap_or(false),
    };

    let mut lepus_code = BTreeMap::new();
    let mut manifest = BTreeMap::new();
    if let Some(range) = routes.get(&SECTION_JS) {
        decode_js_sources(bytes, *range, &mut manifest)?;
    }

    let mut styles = BTreeMap::new();
    if let Some(range) = routes.get(&SECTION_CUSTOM) {
        decode_custom_sections(
            bytes,
            *range,
            css_options,
            &mut lepus_code,
            &mut manifest,
            &mut styles,
        )?;
    }

    let config = build_web_config(page_config, &fields, &target_version_source, &app_type);

    Ok(NativeBundle {
        config,
        lepus_code,
        manifest,
        styles,
    })
}

fn build_web_config(
    page_config: Map<String, JsonValue>,
    fields: &HeaderFields,
    target_version: &str,
    app_type: &str,
) -> Map<String, JsonValue> {
    let mut config = stringify_page_config(page_config);
    add_header_config(&mut config, fields, target_version);
    config.insert("cardType".to_owned(), JsonValue::String("react".to_owned()));
    config.insert(
        "isLazy".to_owned(),
        JsonValue::String((app_type != "card").to_string()),
    );
    config
}

fn decode_header_fields(reader: &mut Reader<'_>) -> Result<HeaderFields, ConvertError> {
    let start = reader.local_position();
    let total_size = usize::try_from(reader.u32()?)
        .map_err(|_| ConvertError::invalid(reader.position(), "header-info size overflow"))?;
    if total_size < 12 {
        return Err(ConvertError::invalid(
            reader.position(),
            "header-info block is shorter than its fixed header",
        ));
    }
    let magic = reader.u32()?;
    if magic != HEADER_INFO_MAGIC {
        return Err(ConvertError::invalid(
            reader.position() - 4,
            format!("bad header-info magic 0x{magic:08x}"),
        ));
    }
    let field_count = usize::try_from(reader.u32()?)
        .map_err(|_| ConvertError::invalid(reader.position(), "field count overflow"))?;
    if field_count > reader.remaining() / 4 {
        return Err(ConvertError::invalid(
            reader.position(),
            "header-info field count exceeds the remaining input",
        ));
    }

    let end = start
        .checked_add(total_size)
        .ok_or_else(|| ConvertError::invalid(reader.position(), "header-info range overflow"))?;
    let mut fields = BTreeMap::new();
    for _ in 0..field_count {
        let field_type = reader.u8()?;
        let key = reader.u8()?;
        let payload_size = usize::from(reader.u16()?);
        let payload = reader.take(payload_size)?.to_vec();
        if fields
            .insert(
                key,
                HeaderField {
                    field_type,
                    payload,
                },
            )
            .is_some()
        {
            return Err(ConvertError::invalid(
                reader.position(),
                format!("duplicate header-info key {key}"),
            ));
        }
    }
    if reader.local_position() > end {
        return Err(ConvertError::invalid(
            reader.position(),
            "header-info fields exceed the declared block size",
        ));
    }
    reader.set_local_position(end)?;
    Ok(HeaderFields(fields))
}

fn decode_section_routes(
    reader: &mut Reader<'_>,
    bundle_len: usize,
) -> Result<HashMap<u8, SectionRange>, ConvertError> {
    let marker_offset = reader.position();
    let marker = reader.u8()?;
    if marker != SECTION_ROUTE {
        return Err(ConvertError::invalid(
            marker_offset,
            format!("expected SECTION_ROUTE ({SECTION_ROUTE}), found {marker}"),
        ));
    }
    let count = usize::try_from(reader.u32()?)
        .map_err(|_| ConvertError::invalid(reader.position(), "section count overflow"))?;
    if count > reader.remaining() / 9 {
        return Err(ConvertError::invalid(
            reader.position(),
            "section route count exceeds the remaining input",
        ));
    }

    let mut relative = Vec::with_capacity(count);
    for _ in 0..count {
        relative.push((reader.u8()?, reader.u32()?, reader.u32()?));
    }
    let descriptor_end = reader.position();
    let mut routes = HashMap::with_capacity(count);
    for (section, start, end) in relative {
        if start > end {
            return Err(ConvertError::invalid(
                descriptor_end,
                format!("section {section} has a reversed range {start}..{end}"),
            ));
        }
        let start = usize::try_from(start)
            .map_err(|_| ConvertError::invalid(descriptor_end, "section start overflow"))?;
        let end = usize::try_from(end)
            .map_err(|_| ConvertError::invalid(descriptor_end, "section end overflow"))?;
        let start = descriptor_end
            .checked_add(start)
            .ok_or_else(|| ConvertError::invalid(descriptor_end, "section start overflow"))?;
        let end = descriptor_end
            .checked_add(end)
            .ok_or_else(|| ConvertError::invalid(descriptor_end, "section end overflow"))?;
        if end > bundle_len {
            return Err(ConvertError::invalid(
                descriptor_end,
                format!("section {section} ends outside the bundle"),
            ));
        }
        if routes
            .insert(section, SectionRange { start, end })
            .is_some()
        {
            return Err(ConvertError::invalid(
                descriptor_end,
                format!("duplicate section route {section}"),
            ));
        }
    }
    Ok(routes)
}

fn reject_top_level_bytecode(
    bytes: &[u8],
    routes: &HashMap<u8, SectionRange>,
    app_type: &str,
) -> Result<(), ConvertError> {
    for (section, name) in [
        (SECTION_JS_BYTECODE, "JS_BYTECODE"),
        (SECTION_LEPUS_CHUNK, "LEPUS_CHUNK"),
    ] {
        if routes.contains_key(&section) {
            return Err(ConvertError::CodeCacheBundle {
                section: name.to_owned(),
            });
        }
    }
    if let Some(range) = routes.get(&SECTION_ROOT_LEPUS) {
        let js_is_empty = routes
            .get(&SECTION_JS)
            .map(|range| is_empty_counted_section(bytes, *range, SECTION_JS, "JS"))
            .transpose()?
            .unwrap_or(true);
        let css_is_empty = routes
            .get(&SECTION_CSS)
            .map(|range| is_empty_counted_section(bytes, *range, SECTION_CSS, "CSS"))
            .transpose()?
            .unwrap_or(true);
        let is_external_layout = app_type == "DynamicComponent"
            && routes.contains_key(&SECTION_CUSTOM)
            && js_is_empty
            && css_is_empty;
        if !is_external_layout || !is_empty_root_lepus(bytes, *range)? {
            return Err(ConvertError::CodeCacheBundle {
                section: "ROOT_LEPUS".to_owned(),
            });
        }
    }
    Ok(())
}

fn is_empty_counted_section(
    bytes: &[u8],
    range: SectionRange,
    section_type: u8,
    name: &'static str,
) -> Result<bool, ConvertError> {
    let mut reader = section_reader(bytes, range, section_type)?;
    let count = reader.u32()?;
    if count != 0 {
        return Ok(false);
    }
    ensure_empty(&reader, name)?;
    Ok(true)
}

fn is_empty_root_lepus(bytes: &[u8], range: SectionRange) -> Result<bool, ConvertError> {
    let mut reader = section_reader(bytes, range, SECTION_ROOT_LEPUS)?;
    let length = usize::try_from(reader.u64()?)
        .map_err(|_| ConvertError::invalid(reader.position(), "ROOT_LEPUS length overflow"))?;
    let bytecode = reader.take(length)?;
    ensure_empty(&reader, "ROOT_LEPUS")?;
    Ok(bytecode == EMPTY_ROOT_LEPUS_INLINE_DEBUG || bytecode == EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG)
}

fn decode_page_config(
    bytes: &[u8],
    range: SectionRange,
) -> Result<Map<String, JsonValue>, ConvertError> {
    let mut reader = section_reader(bytes, range, SECTION_CONFIG)?;
    let source = reader.string("CONFIG JSON")?;
    ensure_empty(&reader, "CONFIG")?;
    if source.is_empty() {
        return Ok(Map::new());
    }
    let value: JsonValue =
        serde_json::from_str(&source).map_err(|source| ConvertError::InvalidJson {
            section: "CONFIG",
            source,
        })?;
    let JsonValue::Object(config) = value else {
        return Err(ConvertError::invalid(
            range.start,
            "CONFIG JSON must be an object",
        ));
    };
    Ok(config)
}

fn decode_js_sources(
    bytes: &[u8],
    range: SectionRange,
    manifest: &mut BTreeMap<String, String>,
) -> Result<(), ConvertError> {
    let mut reader = section_reader(bytes, range, SECTION_JS)?;
    let count = usize::try_from(reader.u32()?)
        .map_err(|_| ConvertError::invalid(reader.position(), "JS file count overflow"))?;
    if count > reader.remaining() / 8 {
        return Err(ConvertError::invalid(
            reader.position(),
            "JS file count exceeds the section payload",
        ));
    }
    for _ in 0..count {
        let path = reader.string("JS path")?;
        let source = reader.string("JS source")?;
        if manifest.insert(path.clone(), source).is_some() {
            return Err(ConvertError::invalid(
                reader.position(),
                format!("duplicate JS path {path:?}"),
            ));
        }
    }
    ensure_empty(&reader, "JS")
}

fn decode_custom_sections(
    bytes: &[u8],
    range: SectionRange,
    css_options: CssOptions,
    lepus_code: &mut BTreeMap<String, String>,
    manifest: &mut BTreeMap<String, String>,
    styles: &mut BTreeMap<i32, StyleSheet>,
) -> Result<(), ConvertError> {
    let mut reader = section_reader(bytes, range, SECTION_CUSTOM)?;
    let headers = decode_custom_headers(&mut reader)?;
    let content_start = reader.position();
    for header in headers {
        decode_custom_content(
            bytes,
            range.end,
            content_start,
            &header,
            css_options,
            lepus_code,
            manifest,
            styles,
        )?;
    }
    Ok(())
}

fn reject_custom_bytecode(bytes: &[u8], range: SectionRange) -> Result<(), ConvertError> {
    let mut reader = section_reader(bytes, range, SECTION_CUSTOM)?;
    let headers = decode_custom_headers(&mut reader)?;
    if let Some(header) = headers
        .iter()
        .find(|header| header.encoding == CUSTOM_JS_BYTECODE)
    {
        return Err(ConvertError::CodeCacheBundle {
            section: header.name.clone(),
        });
    }
    Ok(())
}

fn decode_custom_headers(reader: &mut Reader<'_>) -> Result<Vec<CustomHeader>, ConvertError> {
    let count = usize::try_from(reader.u32()?)
        .map_err(|_| ConvertError::invalid(reader.position(), "custom-section count overflow"))?;
    if count > reader.remaining() / 13 {
        return Err(ConvertError::invalid(
            reader.position(),
            "custom-section count exceeds the section payload",
        ));
    }

    let mut headers = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader.string("custom-section name")?;
        let header = decode_value(reader, 0)?;
        let encoding = header
            .table()
            .and_then(|table| table.get("encoding"))
            .and_then(LepusValue::number_i32)
            .unwrap_or(CUSTOM_SOURCE);
        let start = usize::try_from(reader.u32()?).map_err(|_| {
            ConvertError::invalid(reader.position(), "custom-section start overflow")
        })?;
        let end = usize::try_from(reader.u32()?)
            .map_err(|_| ConvertError::invalid(reader.position(), "custom-section end overflow"))?;
        if start > end {
            return Err(ConvertError::invalid(
                reader.position(),
                format!("custom section {name:?} has a reversed range"),
            ));
        }
        headers.push(CustomHeader {
            name,
            encoding,
            start,
            end,
        });
    }
    Ok(headers)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the arguments are the distinct output maps and section bounds being populated"
)]
fn decode_custom_content(
    bytes: &[u8],
    section_end: usize,
    content_start: usize,
    header: &CustomHeader,
    css_options: CssOptions,
    lepus_code: &mut BTreeMap<String, String>,
    manifest: &mut BTreeMap<String, String>,
    styles: &mut BTreeMap<i32, StyleSheet>,
) -> Result<(), ConvertError> {
    let start = content_start
        .checked_add(header.start)
        .ok_or_else(|| ConvertError::invalid(content_start, "custom-section start overflow"))?;
    let end = content_start
        .checked_add(header.end)
        .ok_or_else(|| ConvertError::invalid(content_start, "custom-section end overflow"))?;
    if end > section_end {
        return Err(ConvertError::invalid(
            content_start,
            format!(
                "custom section {:?} ends outside CUSTOM_SECTIONS",
                header.name
            ),
        ));
    }
    let mut content = Reader::section(bytes, start, end)?;
    match header.encoding {
        CUSTOM_SOURCE => {
            let value = decode_value(&mut content, 0)?;
            ensure_empty(&content, "custom source")?;
            let LepusValue::String(source) = value else {
                return Err(ConvertError::UnsupportedBundle(format!(
                    "custom section {:?} is not JavaScript source text",
                    header.name
                )));
            };
            if header.name.ends_with("__main-thread") {
                if lepus_code.insert(header.name.clone(), source).is_some() {
                    return Err(ConvertError::invalid(
                        start,
                        format!("duplicate main-thread section {:?}", header.name),
                    ));
                }
            } else {
                let path = format!("/{}", header.name);
                if manifest.insert(path.clone(), source).is_some() {
                    return Err(ConvertError::invalid(
                        start,
                        format!("duplicate background script path {path:?}"),
                    ));
                }
            }
        }
        CUSTOM_CSS => {
            let value = decode_value(&mut content, 0)?;
            ensure_empty(&content, "custom CSS")?;
            let LepusValue::ByteArray(fragment) = value else {
                return Err(ConvertError::invalid(
                    start,
                    format!("CSS custom section {:?} is not a byte array", header.name),
                ));
            };
            let (id, style_sheet) = decode_fragment(&fragment, css_options)?;
            if styles.insert(id, style_sheet).is_some() {
                return Err(ConvertError::invalid(
                    start,
                    format!("duplicate native CSS fragment id {id}"),
                ));
            }
        }
        other => {
            return Err(ConvertError::UnsupportedBundle(format!(
                "custom section {:?} uses unknown encoding {other}",
                header.name
            )));
        }
    }
    Ok(())
}

fn section_reader(
    bytes: &[u8],
    range: SectionRange,
    expected: u8,
) -> Result<Reader<'_>, ConvertError> {
    let mut reader = Reader::section(bytes, range.start, range.end)?;
    let offset = reader.position();
    let actual = reader.u8()?;
    if actual != expected {
        return Err(ConvertError::invalid(
            offset,
            format!("route expects section {expected}, payload starts with {actual}"),
        ));
    }
    Ok(reader)
}

fn ensure_empty(reader: &Reader<'_>, section: &'static str) -> Result<(), ConvertError> {
    if reader.is_empty() {
        Ok(())
    } else {
        Err(ConvertError::invalid(
            reader.position(),
            format!(
                "{section} section has {} trailing bytes",
                reader.remaining()
            ),
        ))
    }
}

pub(crate) fn decode_value(
    reader: &mut Reader<'_>,
    depth: usize,
) -> Result<LepusValue, ConvertError> {
    if depth >= VALUE_RECURSION_LIMIT {
        return Err(ConvertError::invalid(
            reader.position(),
            "Lepus value nesting limit exceeded",
        ));
    }
    let tag_offset = reader.position();
    let tag = reader.u8()?;
    match tag {
        0 => Ok(LepusValue::Nil),
        1 => Ok(LepusValue::Number(reader.f64()?)),
        2 => Ok(LepusValue::Bool(reader.u8()? != 0)),
        3 => Ok(LepusValue::String(reader.string("Lepus string")?)),
        4 => {
            let count = usize::try_from(reader.u32()?).map_err(|_| {
                ConvertError::invalid(reader.position(), "Lepus table size overflow")
            })?;
            if count > reader.remaining() / 5 {
                return Err(ConvertError::invalid(
                    reader.position(),
                    "Lepus table size exceeds the remaining input",
                ));
            }
            let mut table = BTreeMap::new();
            for _ in 0..count {
                let key = reader.string("Lepus table key")?;
                let value = decode_value(reader, depth + 1)?;
                if table.insert(key.clone(), value).is_some() {
                    return Err(ConvertError::invalid(
                        reader.position(),
                        format!("duplicate Lepus table key {key:?}"),
                    ));
                }
            }
            Ok(LepusValue::Table(table))
        }
        5 => {
            let count = usize::try_from(reader.u32()?).map_err(|_| {
                ConvertError::invalid(reader.position(), "Lepus array size overflow")
            })?;
            if count > reader.remaining() {
                return Err(ConvertError::invalid(
                    reader.position(),
                    "Lepus array size exceeds the remaining input",
                ));
            }
            let mut array = Vec::with_capacity(count);
            for _ in 0..count {
                array.push(decode_value(reader, depth + 1)?);
            }
            Ok(LepusValue::Array(array))
        }
        9 => Ok(LepusValue::Number(f64::from(reader.i32()?))),
        10 => {
            let raw = reader.u64()?;
            #[expect(
                clippy::cast_precision_loss,
                reason = "the native Lepus value itself stores this integer through a Number API"
            )]
            Ok(LepusValue::Number(raw.cast_signed() as f64))
        }
        11 => Ok(LepusValue::Number(f64::from(reader.u32()?))),
        12 => {
            let raw = reader.u64()?;
            #[expect(
                clippy::cast_precision_loss,
                reason = "the native Lepus value itself stores this integer through a Number API"
            )]
            Ok(LepusValue::Number(raw as f64))
        }
        13 => {
            let _nan_flag = reader.u8()?;
            Ok(LepusValue::Number(f64::NAN))
        }
        14 => {
            for _ in 0..12 {
                let _component = reader.i32()?;
            }
            Ok(LepusValue::Undefined)
        }
        15 => {
            let _pattern = decode_value(reader, depth + 1)?;
            let _flags = decode_value(reader, depth + 1)?;
            Ok(LepusValue::Undefined)
        }
        17 => Ok(LepusValue::Undefined),
        18 => {
            let length = usize::try_from(reader.u64()?).map_err(|_| {
                ConvertError::invalid(reader.position(), "byte-array length overflow")
            })?;
            Ok(LepusValue::ByteArray(reader.take(length)?.to_vec()))
        }
        other => Err(ConvertError::invalid(
            tag_offset,
            format!("unsupported Lepus value type {other}"),
        )),
    }
}

fn config_flag(config: &Map<String, JsonValue>, key: &str) -> bool {
    config.get(key).is_some_and(|value| match value {
        JsonValue::Bool(value) => *value,
        JsonValue::String(value) => value == "true",
        _ => false,
    })
}

fn stringify_page_config(config: Map<String, JsonValue>) -> Map<String, JsonValue> {
    config
        .into_iter()
        .map(|(key, value)| (key, JsonValue::String(config_value_string(&value))))
        .collect()
}

fn config_value_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_owned(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::String(value) => value.clone(),
        JsonValue::Array(_) | JsonValue::Object(_) => value.to_string(),
    }
}

fn add_header_config(
    config: &mut Map<String, JsonValue>,
    fields: &HeaderFields,
    target_version: &str,
) {
    config
        .entry("targetSdkVersion".to_owned())
        .or_insert_with(|| JsonValue::String(target_version.to_owned()));

    for (key, name) in [
        (1, "enableCSSParser"),
        (2, "enableCSSExternalClass"),
        (3, "enableCSSStrictMode"),
        (4, "useLepusNG"),
        (5, "defaultOverflowVisible"),
        (6, "enableCSSVariable"),
        (7, "defaultImplicitAnimation"),
        (10, "enableKeepPageData"),
        (11, "enableRemoveCSSScope"),
        (13, "enableCSSClassMerge"),
        (14, "defaultDisplayLinear"),
        (15, "removeCSSParserLog"),
        (16, "enableLynxAir"),
        (20, "enableTrialOptions"),
        (22, "enableCSSEngine"),
        (23, "enableComponentConfig"),
        (25, "enableFiberArch"),
        (26, "debugInfoOutside"),
        (27, "enableFlexibleTemplate"),
        (29, "enableCSSSelector"),
        (30, "enableReuseContext"),
        (31, "enableCSSInvalidation"),
        (32, "enableAsyncLepusChunkDecode"),
        (33, "enableSimpleStyling"),
    ] {
        if let Some(value) = fields.bool(key) {
            config
                .entry(name.to_owned())
                .or_insert_with(|| JsonValue::String(value.to_string()));
        }
    }

    for (key, name) in [(8, "radonMode"), (9, "frontEndDSL")] {
        if let Some(value) = fields.i32(key) {
            config
                .entry(name.to_owned())
                .or_insert_with(|| JsonValue::String(value.to_string()));
        }
    }
    if let Some(value) = fields.string(12)
        && !value.is_empty()
    {
        config
            .entry("templateDebugUrl".to_owned())
            .or_insert_with(|| JsonValue::String(value.to_owned()));
    }
}
