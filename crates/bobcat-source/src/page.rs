//! Turns already-loaded Lynx page inputs into Bobcat view sources.
//!
//! [`PageSource::from_bytes`] sniffs and decodes either a web bundle or strict
//! UTF-8 Lynx XML for native embedders. [`register_lynx_xml_response`] is the
//! one-shot browser path: it accepts text decoded under browser policy, maps
//! sections to fragments of the final response URL, and registers only the
//! sources a view can load. Filesystem, network IO, and byte-to-text policy
//! remain the caller's responsibility.

use std::fmt;
use std::sync::Arc;

#[cfg(feature = "web-bundle")]
use bobcat_core::PreparsedStyleSheet;
use bobcat_core::{PageConfig, ViewSources};
use bobcat_resources::Resources;
use thiserror::Error;
use url::Url;

/// A decoded page container and the in-memory resources it contributes to a
/// Bobcat view.
#[derive(Clone, Debug)]
pub struct PageSource {
    input_url: Url,
    script_url: Url,
    script: Arc<str>,
    background_script: Option<(Url, Arc<str>)>,
    style_sheet: Option<(Url, PageStyleSheet)>,
    config: PageConfig,
    compatibility_warnings: Vec<CompatibilityWarning>,
}

/// The view-facing result of registering one browser-loaded Lynx XML response.
///
/// Main-thread script and author CSS bytes have already been copied into the
/// supplied resource registry. A background-thread section is named and
/// reported, but its body is neither retained here nor registered because the
/// current runtime cannot execute it.
#[derive(Debug)]
pub struct LynxXmlResponseRegistration {
    entry_url: Url,
    style_sheet_url: Option<Url>,
    background_thread_url: Option<Url>,
    compatibility_warnings: Vec<String>,
}

impl LynxXmlResponseRegistration {
    /// The registered main-thread module URL.
    #[must_use]
    pub const fn entry_url(&self) -> &Url {
        &self.entry_url
    }

    /// The registered author stylesheet URL, when the section was present.
    #[must_use]
    pub const fn style_sheet_url(&self) -> Option<&Url> {
        self.style_sheet_url.as_ref()
    }

    /// The logical URL of a present, unsupported background-thread section.
    #[must_use]
    pub const fn background_thread_url(&self) -> Option<&Url> {
        self.background_thread_url.as_ref()
    }

    /// Input features handled only approximately or not executed.
    #[must_use]
    pub fn compatibility_warnings(&self) -> &[String] {
        &self.compatibility_warnings
    }
}

/// A page feature retained by the source loader but not fully supported by
/// the current runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompatibilityWarning {
    /// Rules from these component CSS fragments currently mount globally.
    ComponentScopedCss { css_ids: Vec<i32> },
    /// This raw XML background script is retained but is not executed.
    BackgroundThreadScript { url: Url },
}

impl fmt::Display for CompatibilityWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentScopedCss { css_ids } => {
                let mut ids = css_ids.iter();
                write!(formatter, "component-scoped CSS fragments (css ids ")?;
                if let Some(id) = ids.next() {
                    write!(formatter, "{id}")?;
                    for id in ids {
                        write!(formatter, ", {id}")?;
                    }
                }
                write!(
                    formatter,
                    "); per-component scoping is not implemented, so their rules apply globally"
                )
            }
            Self::BackgroundThreadScript { url } => {
                write!(
                    formatter,
                    "a Lynx XML background-thread script at {}; background-thread JavaScript is retained but not executed",
                    section_url_label(url)
                )
            }
        }
    }
}

/// Failure to classify or decode a page container.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SourceError {
    #[cfg(feature = "web-bundle")]
    #[error("could not decode web bundle `{input}`: {source}")]
    DecodeWebBundle {
        input: String,
        #[source]
        source: crate::web::DecodeError,
    },
    #[error("Lynx XML `{input}` is not valid UTF-8: {source}")]
    InvalidLynxXmlEncoding {
        input: String,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("could not parse Lynx XML `{input}`: {source}")]
    ParseLynxXml {
        input: String,
        #[source]
        source: crate::xml::ParseError,
    },
    #[error("web bundle `{0}` has no `lepusCode.root` entry")]
    #[cfg(feature = "web-bundle")]
    MissingRoot(String),
    #[cfg(not(feature = "web-bundle"))]
    #[error("web bundle support is disabled for `{0}`")]
    WebBundleSupportDisabled(String),
    #[cfg(feature = "native-bundle")]
    #[error("could not decode native bundle `{input}`: {source}")]
    DecodeNativeBundle {
        input: String,
        #[source]
        source: crate::native::ConvertError,
    },
    #[cfg(feature = "native-bundle")]
    #[error(
        "native bundle `{input}` has no main-thread module `{entry}`; select an external module explicitly"
    )]
    MissingNativeEntry { input: String, entry: String },
    #[cfg(not(feature = "native-bundle"))]
    #[error("native Lynx bundle support is disabled for `{0}`")]
    UnsupportedNativeBundle(String),
}

/// Explicit name for [`SourceError`] at APIs that carry several error types.
pub type PageSourceError = SourceError;

/// Author CSS in the most direct form supplied by the input container.
#[derive(Clone, Debug)]
enum PageStyleSheet {
    /// Verbatim UTF-8 CSS from a raw Lynx XML `<style>` section.
    Text(Arc<str>),
    /// A web bundle's rkyv `StyleInfo`, lowered without reserializing it.
    #[cfg(feature = "web-bundle")]
    Preparsed(PreparsedStyleSheet),
}

#[derive(Debug)]
struct LynxXmlSectionUrls {
    main_thread: Url,
    style: Url,
    background_thread: Url,
}

#[derive(Clone, Copy, Debug)]
enum LynxXmlUrlPolicy {
    InMemory,
    ResponseFragments,
}

#[derive(Debug)]
struct MappedLynxXml<'source> {
    main_thread: (Url, &'source str),
    style: Option<(Url, &'source str)>,
    background_thread: Option<(Url, &'source str)>,
}

impl MappedLynxXml<'_> {
    fn compatibility_warnings(&self) -> Vec<CompatibilityWarning> {
        self.background_thread
            .as_ref()
            .map(|(url, _)| CompatibilityWarning::BackgroundThreadScript { url: url.clone() })
            .into_iter()
            .collect()
    }
}

impl LynxXmlSectionUrls {
    fn in_memory() -> Self {
        Self {
            main_thread: Url::parse("bobcat-memory://lynx-xml/main-thread.js")
                .expect("the built-in XML main-script URL must be valid"),
            style: Url::parse("bobcat-memory://lynx-xml/style.css")
                .expect("the built-in XML stylesheet URL must be valid"),
            background_thread: Url::parse("bobcat-memory://lynx-xml/app-service.js")
                .expect("the built-in XML background-script URL must be valid"),
        }
    }

    fn response_fragments(input: &Url) -> Self {
        Self {
            main_thread: xml_section_url(input, "main-thread"),
            style: xml_section_url(input, "style"),
            background_thread: xml_section_url(input, "background-thread"),
        }
    }
}

impl PageSource {
    /// Decodes a page container that the caller has already loaded.
    pub fn from_bytes(input: &Url, bytes: &[u8]) -> Result<Self, SourceError> {
        if looks_like_native_bundle(bytes) {
            #[cfg(feature = "native-bundle")]
            {
                Self::from_native_bundle(input, bytes, "root")
            }
            #[cfg(not(feature = "native-bundle"))]
            {
                Err(SourceError::UnsupportedNativeBundle(diagnostic_url(input)))
            }
        } else if looks_like_lynx_xml(bytes) {
            let source = std::str::from_utf8(bytes).map_err(|source| {
                SourceError::InvalidLynxXmlEncoding {
                    input: diagnostic_url(input),
                    source,
                }
            })?;
            Self::from_lynx_xml(input, source)
        } else {
            #[cfg(feature = "web-bundle")]
            {
                Self::from_web_bundle(input, bytes)
            }
            #[cfg(not(feature = "web-bundle"))]
            {
                Err(SourceError::WebBundleSupportDisabled(diagnostic_url(input)))
            }
        }
    }

    #[cfg(feature = "web-bundle")]
    fn from_web_bundle(input: &Url, bytes: &[u8]) -> Result<Self, SourceError> {
        let template =
            crate::web::decode(bytes).map_err(|source| SourceError::DecodeWebBundle {
                input: diagnostic_url(input),
                source,
            })?;
        Self::from_template(input, template)
    }

    /// Decode a source-based native external bundle and select its named main module.
    /// The ordinary byte-sniffing path requires `root`; external libraries normally
    /// need an explicit name such as `library__main-thread` instead.
    #[cfg(feature = "native-bundle")]
    pub fn from_native_bundle(input: &Url, bytes: &[u8], entry: &str) -> Result<Self, SourceError> {
        let mut template =
            crate::native::decode(bytes).map_err(|source| SourceError::DecodeNativeBundle {
                input: diagnostic_url(input),
                source,
            })?;
        let source =
            template
                .lepus_code
                .remove(entry)
                .ok_or_else(|| SourceError::MissingNativeEntry {
                    input: diagnostic_url(input),
                    entry: bounded_diagnostic(entry.to_owned()),
                })?;
        template.lepus_code.insert("root".to_owned(), source);
        Self::from_template(input, template)
    }

    #[cfg(feature = "web-bundle")]
    fn from_template(
        input: &Url,
        mut template: crate::web::WebTemplate,
    ) -> Result<Self, SourceError> {
        let source = template
            .lepus_code
            .remove("root")
            .ok_or_else(|| SourceError::MissingRoot(diagnostic_url(input)))?;
        let script_url = Url::parse("bobcat-memory://bundle/lepus-root.js")
            .expect("the built-in root-script URL must be valid");
        let style_sheet = template
            .style_info
            .as_ref()
            .map(crate::lower_style::to_preparsed_style_sheet)
            .filter(|sheet| !sheet.is_empty())
            .map(|sheet| {
                let url = Url::parse("bobcat-memory://bundle/style-info.css")
                    .expect("the built-in stylesheet URL must be valid");
                (url, PageStyleSheet::Preparsed(sheet))
            });
        let scoped_css_ids = template
            .style_info
            .as_ref()
            .map_or_else(Vec::new, |style_info| {
                let mut ids: Vec<i32> = style_info
                    .css_id_to_style_sheet
                    .keys()
                    .copied()
                    .filter(|id| *id != 0)
                    .collect();
                ids.sort_unstable();
                ids
            });
        let config = PageConfig {
            default_display_linear: template.config_flag("defaultDisplayLinear"),
            default_overflow_visible: template.config_flag("defaultOverflowVisible"),
            enable_css_selector: template.config_flag("enableCSSSelector"),
        };
        let compatibility_warnings = if scoped_css_ids.is_empty() {
            Vec::new()
        } else {
            vec![CompatibilityWarning::ComponentScopedCss {
                css_ids: scoped_css_ids,
            }]
        };
        Ok(Self {
            input_url: input.clone(),
            script_url,
            script: Arc::from(source),
            background_script: None,
            style_sheet,
            config,
            compatibility_warnings,
        })
    }

    fn from_lynx_xml(input: &Url, source: &str) -> Result<Self, SourceError> {
        let mapped = map_lynx_xml(input, source, LynxXmlUrlPolicy::InMemory)?;
        let compatibility_warnings = mapped.compatibility_warnings();
        let background_script = mapped
            .background_thread
            .map(|(url, source)| (url, Arc::from(source)));
        let style_sheet = mapped
            .style
            .map(|(url, source)| (url, PageStyleSheet::Text(Arc::from(source))));

        Ok(Self {
            input_url: input.clone(),
            script_url: mapped.main_thread.0,
            script: Arc::from(mapped.main_thread.1),
            background_script,
            style_sheet,
            config: raw_lynx_xml_config(),
            compatibility_warnings,
        })
    }

    /// The URL that identifies the decoded input.
    #[must_use]
    pub const fn input_url(&self) -> &Url {
        &self.input_url
    }

    /// Page policy decoded from this input's configuration.
    #[must_use]
    pub const fn config(&self) -> PageConfig {
        self.config
    }

    /// Registers the decoded scripts and author stylesheet with an
    /// embedder-owned resource system.
    ///
    /// Every registration URL was derived from an already-parsed [`Url`], so
    /// registration cannot fail URL validation.
    pub fn register_with(&self, resources: &Resources) {
        register_text(
            resources,
            &self.script_url,
            &self.script,
            "text/javascript; charset=utf-8",
        );
        if let Some((url, source)) = self.background_script.as_ref() {
            register_text(resources, url, source, "text/javascript; charset=utf-8");
        }
        match self.style_sheet.as_ref() {
            Some((url, PageStyleSheet::Text(source))) => {
                register_text(resources, url, source, "text/css; charset=utf-8");
            }
            #[cfg(feature = "web-bundle")]
            Some((url, PageStyleSheet::Preparsed(sheet))) => {
                resources
                    .register_style_sheet(url.as_str(), sheet.clone())
                    .expect("PageSource's registration URLs are valid");
            }
            None => {}
        }
    }

    /// The sources a view for this input is built from: the author CSS this
    /// input carried, if any, and its entry MTS module.
    #[must_use]
    pub fn view_sources(&self) -> ViewSources {
        ViewSources {
            config: self.config,
            style_sheets: self
                .style_sheet
                .as_ref()
                .map(|(url, _)| url)
                .map(Url::to_string)
                .into_iter()
                .collect(),
            ..ViewSources::new(self.script_url.to_string())
        }
    }

    /// Input features retained only approximately or not executed by the
    /// current runtime.
    #[must_use]
    pub fn compatibility_warnings(&self) -> &[CompatibilityWarning] {
        &self.compatibility_warnings
    }
}

/// Parses and registers one already-decoded browser Lynx XML response.
///
/// Section identities are fragments of `input`, which must be the final
/// response URL so relative imports and CSS URLs retain the browser-observed
/// redirect base. `source` is already Unicode: replacement characters emitted
/// by the browser's UTF-8 decoder are ordinary contents here. Only main-thread
/// script and author CSS are copied into `resources`; a present background
/// body is deliberately not copied because the runtime cannot load it.
pub fn register_lynx_xml_response(
    input: &Url,
    source: &str,
    resources: &Resources,
) -> Result<LynxXmlResponseRegistration, SourceError> {
    let mapped = map_lynx_xml(input, source, LynxXmlUrlPolicy::ResponseFragments)?;
    let compatibility_warnings = mapped
        .background_thread
        .as_ref()
        .map(|(url, _)| {
            format!(
                "a Lynx XML background-thread script at {}; background-thread execution is not implemented",
                section_url_label(url)
            )
        })
        .into_iter()
        .collect();
    let background_thread_url = mapped
        .background_thread
        .as_ref()
        .map(|(url, _)| url.clone());
    let (entry_url, main_thread_script) = mapped.main_thread;
    register_text(
        resources,
        &entry_url,
        main_thread_script,
        "text/javascript; charset=utf-8",
    );

    let style_sheet_url = mapped.style.map(|(url, style)| {
        register_text(resources, &url, style, "text/css; charset=utf-8");
        url
    });

    Ok(LynxXmlResponseRegistration {
        entry_url,
        style_sheet_url,
        background_thread_url,
        compatibility_warnings,
    })
}

fn map_lynx_xml<'source>(
    input: &Url,
    source: &'source str,
    url_policy: LynxXmlUrlPolicy,
) -> Result<MappedLynxXml<'source>, SourceError> {
    let xml = crate::xml::parse(source).map_err(|source| SourceError::ParseLynxXml {
        input: diagnostic_url(input),
        source,
    })?;
    // Parse first: a malformed response, especially a large `data:` URL, must
    // not pay to clone three section identities that will never be used.
    let section_urls = match url_policy {
        LynxXmlUrlPolicy::InMemory => LynxXmlSectionUrls::in_memory(),
        LynxXmlUrlPolicy::ResponseFragments => LynxXmlSectionUrls::response_fragments(input),
    };
    let LynxXmlSectionUrls {
        main_thread,
        style,
        background_thread,
    } = section_urls;
    Ok(MappedLynxXml {
        main_thread: (main_thread, xml.main_thread_script),
        style: xml.style.map(|source| (style, source)),
        background_thread: xml
            .background_thread_script
            .map(|source| (background_thread, source)),
    })
}

const fn raw_lynx_xml_config() -> PageConfig {
    PageConfig {
        default_display_linear: false,
        default_overflow_visible: false,
        enable_css_selector: true,
    }
}

fn register_text(resources: &Resources, url: &Url, source: &str, media_type: &str) {
    resources
        .register(url.as_str(), source.as_bytes().to_vec(), Some(media_type))
        .expect("PageSource's registration URLs are valid");
}

fn xml_section_url(input: &Url, fragment: &str) -> Url {
    let mut url = input.clone();
    url.set_fragment(Some(fragment));
    url
}

fn diagnostic_url(input: &Url) -> String {
    if input.cannot_be_a_base() {
        return format!("{}:[redacted]", input.scheme());
    }
    let mut redacted = input.clone();
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    redacted.set_query(None);
    redacted.set_fragment(None);
    bounded_diagnostic(redacted.to_string())
}

fn section_url_label(url: &Url) -> String {
    let mut label = if url.cannot_be_a_base() {
        format!("{}:[redacted]", url.scheme())
    } else {
        url.path().to_owned()
    };
    if let Some(fragment) = url.fragment() {
        label.push('#');
        label.push_str(fragment);
    }
    bounded_diagnostic(label)
}

fn bounded_diagnostic(mut value: String) -> String {
    const MAX_BYTES: usize = 256;
    if value.len() <= MAX_BYTES {
        return value;
    }
    let mut end = MAX_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push('…');
    value
}

/// Recognizes native magics before the input can fall through to web decoding.
/// The native decoder validates the leading total size and section structure.
fn looks_like_native_bundle(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..8) else {
        return false;
    };
    let magic = u32::from_le_bytes(header[4..].try_into().expect("four-byte native magic"));
    matches!(magic, 0x0024_1922 | 0xdd73_7199)
}

/// Mirrors web-core's raw-input classification: any run of ASCII whitespace
/// and UTF-8 BOMs is ignored for sniffing, while the XML parser itself remains
/// responsible for enforcing its stricter single-leading-BOM grammar.
fn looks_like_lynx_xml(mut bytes: &[u8]) -> bool {
    const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

    loop {
        let original_len = bytes.len();
        bytes = bytes.trim_ascii_start();
        if let Some(rest) = bytes.strip_prefix(UTF8_BOM) {
            bytes = rest;
        }
        if bytes.len() == original_len {
            break;
        }
    }
    bytes.first() == Some(&b'<')
}

#[cfg(test)]
mod tests {
    use bobcat_resources::ResourcesConfig;

    use super::*;

    fn input_url() -> Url {
        Url::parse("file:///tmp/card.lynx.xml").expect("test URL")
    }

    fn resources() -> Resources {
        Resources::new(
            ResourcesConfig {
                worker_threads: 1,
                log_to_stderr: false,
                ..ResourcesConfig::default()
            },
            || {},
        )
    }

    #[cfg(feature = "web-bundle")]
    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    #[cfg(feature = "web-bundle")]
    fn push_section(bytes: &mut Vec<u8>, label: u32, content: &[u8]) {
        push_u32(bytes, label);
        push_u32(
            bytes,
            u32::try_from(content.len()).expect("tiny test section"),
        );
        bytes.extend_from_slice(content);
    }

    #[cfg(feature = "web-bundle")]
    fn push_string(bytes: &mut Vec<u8>, value: &str) {
        push_u32(bytes, u32::try_from(value.len()).expect("tiny test string"));
        bytes.extend_from_slice(value.as_bytes());
    }

    #[cfg(feature = "web-bundle")]
    fn web_bundle(root: Option<&str>) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, crate::web::MAGIC_0);
        push_u32(&mut bytes, crate::web::MAGIC_1);
        push_u32(&mut bytes, 1);

        let config = r#"{
            "defaultDisplayLinear": "true",
            "defaultOverflowVisible": "false",
            "enableCSSSelector": "true"
        }"#;
        let config: Vec<u8> = config.encode_utf16().flat_map(u16::to_le_bytes).collect();
        push_section(
            &mut bytes,
            crate::web::SectionLabel::Configurations as u32,
            &config,
        );

        let mut lepus = Vec::new();
        push_u32(&mut lepus, u32::from(root.is_some()));
        if let Some(root) = root {
            push_string(&mut lepus, "root");
            push_string(&mut lepus, root);
        }
        push_section(
            &mut bytes,
            crate::web::SectionLabel::LepusCode as u32,
            &lepus,
        );
        bytes
    }

    #[test]
    fn sniff_ignores_ascii_whitespace_and_utf8_boms() {
        assert!(looks_like_lynx_xml(b"<lynx"));
        assert!(looks_like_lynx_xml(b"\xef\xbb\xbf \n\t<lynx"));
        assert!(looks_like_lynx_xml(b" \xef\xbb\xbf\n\xef\xbb\xbf<lynx"));
        assert!(!looks_like_lynx_xml(b"SDRA WROF"));
        assert!(!looks_like_lynx_xml(b"  {\"json\":true}"));
    }

    #[cfg(feature = "web-bundle")]
    #[test]
    fn web_bundle_exposes_config_entry_and_input_url() {
        let input = Url::parse("https://example.test/card.web.bundle").expect("test URL");
        let page = PageSource::from_bytes(&input, &web_bundle(Some("export {};")))
            .expect("valid web bundle");

        assert_eq!(page.input_url(), &input);
        assert_eq!(
            page.config(),
            PageConfig {
                default_display_linear: true,
                default_overflow_visible: false,
                enable_css_selector: true,
            }
        );
        let sources = page.view_sources();
        assert_eq!(sources.config, page.config());
        assert_eq!(sources.entry, "bobcat-memory://bundle/lepus-root.js");
        assert!(sources.style_sheets.is_empty());
        assert!(page.compatibility_warnings().is_empty());

        let resources = resources();
        page.register_with(&resources);
        assert!(resources.unregister(&sources.entry));
    }

    #[cfg(feature = "web-bundle")]
    #[test]
    fn web_bundle_requires_a_root_script() {
        let error = PageSource::from_bytes(&input_url(), &web_bundle(None))
            .expect_err("a bundle without root code must fail");

        assert!(matches!(error, SourceError::MissingRoot(_)));
    }

    #[test]
    fn xml_preserves_present_empty_style_and_background_sections() {
        let page = PageSource::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><style></style><script thread=\"main\">main</script><script thread=\"background\"></script></lynx>",
        )
        .expect("valid XML page");

        assert!(!page.config().default_display_linear);
        assert!(!page.config().default_overflow_visible);
        assert!(page.config().enable_css_selector);
        assert_eq!(page.input_url(), &input_url());
        assert_eq!(
            page.style_sheet.as_ref().map(|(url, _)| url.as_str()),
            Some("bobcat-memory://lynx-xml/style.css")
        );
        assert_eq!(
            page.compatibility_warnings(),
            &[CompatibilityWarning::BackgroundThreadScript {
                url: Url::parse("bobcat-memory://lynx-xml/app-service.js").expect("test URL"),
            }]
        );
        assert!(matches!(
            page.style_sheet.as_ref(),
            Some((_, PageStyleSheet::Text(source))) if source.is_empty()
        ));
        assert!(matches!(
            page.background_script.as_ref(),
            Some((_, source)) if source.is_empty()
        ));
        let sources = page.view_sources();
        assert_eq!(sources.config, page.config());
        assert_eq!(sources.entry, "bobcat-memory://lynx-xml/main-thread.js");
        assert_eq!(
            sources.style_sheets,
            vec!["bobcat-memory://lynx-xml/style.css".to_owned()]
        );

        let resources = resources();
        page.register_with(&resources);
        assert!(resources.unregister(&sources.entry));
        assert!(resources.unregister("bobcat-memory://lynx-xml/app-service.js"));
        assert!(resources.unregister(&sources.style_sheets[0]));
    }

    #[test]
    fn browser_response_maps_and_registers_only_view_sources() {
        let input = Url::parse("https://cdn.example/final/card.lynx.xml?revision=2#request")
            .expect("test URL");
        let resources = resources();
        let registered = register_lynx_xml_response(
            &input,
            "<lynx engine-version=\"4.2\"><style></style><script thread=\"main\">main</script><script thread=\"background\">unused</script></lynx>",
            &resources,
        )
        .expect("valid browser XML response");

        assert_eq!(
            registered.entry_url().as_str(),
            "https://cdn.example/final/card.lynx.xml?revision=2#main-thread"
        );
        assert_eq!(
            registered.style_sheet_url().map(Url::as_str),
            Some("https://cdn.example/final/card.lynx.xml?revision=2#style")
        );
        assert_eq!(
            registered.background_thread_url().map(Url::as_str),
            Some("https://cdn.example/final/card.lynx.xml?revision=2#background-thread")
        );
        assert_eq!(
            registered.compatibility_warnings(),
            &[
                "a Lynx XML background-thread script at /final/card.lynx.xml#background-thread; background-thread execution is not implemented"
            ]
        );
        assert!(resources.unregister(registered.entry_url().as_str()));
        assert!(
            resources.unregister(
                registered
                    .style_sheet_url()
                    .expect("present style URL")
                    .as_str()
            )
        );
        assert!(
            !resources.unregister(
                registered
                    .background_thread_url()
                    .expect("present background URL")
                    .as_str()
            )
        );
    }

    #[test]
    fn xml_without_optional_sections_keeps_them_absent() {
        let page = PageSource::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">main</script></lynx>",
        )
        .expect("valid XML page");

        assert!(page.style_sheet.is_none());
        assert!(page.background_script.is_none());
        assert!(page.compatibility_warnings().is_empty());
        assert_eq!(
            page.view_sources().entry,
            "bobcat-memory://lynx-xml/main-thread.js"
        );
    }

    #[test]
    fn browser_decoded_xml_accepts_replacement_characters() {
        let resources = resources();
        let registered = register_lynx_xml_response(
            &input_url(),
            "<lynx engine-version=\"4.2\"><script thread=\"main\">const decoded = '\u{fffd}';</script></lynx>",
            &resources,
        )
        .expect("a browser replacement character is valid Unicode source");

        assert_eq!(
            registered.entry_url().as_str(),
            "file:///tmp/card.lynx.xml#main-thread"
        );
        assert!(resources.unregister(registered.entry_url().as_str()));
    }

    #[test]
    fn sniffed_xml_rejects_invalid_utf8_before_parsing() {
        let error = PageSource::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">\xff</script></lynx>",
        )
        .expect_err("invalid UTF-8 must fail");

        assert!(matches!(error, SourceError::InvalidLynxXmlEncoding { .. }));
    }

    #[test]
    fn malformed_sniffed_xml_reports_an_xml_parse_error() {
        let error = PageSource::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">main</script>",
        )
        .expect_err("malformed XML must fail");

        assert!(matches!(error, SourceError::ParseLynxXml { .. }));
    }

    #[test]
    fn malformed_browser_xml_does_not_expose_url_credentials() {
        let input =
            Url::parse("https://user:secret@example.test/card.lynx.xml?token=secret#request")
                .expect("test URL");
        let error = register_lynx_xml_response(
            &input,
            "<lynx engine-version=\"4.2\"><script thread=\"main\">main</script>",
            &resources(),
        )
        .expect_err("malformed XML must fail");
        let message = error.to_string();

        assert!(message.contains("https://example.test/card.lynx.xml"));
        assert!(!message.contains("user"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("token"));
        assert!(!message.contains("request"));
    }

    #[test]
    fn opaque_urls_are_redacted_in_errors_and_warnings() {
        let input = Url::parse("data:text/xml,super-secret-source").expect("test data URL");
        let error = register_lynx_xml_response(&input, "<lynx", &resources())
            .expect_err("malformed XML must fail");
        assert!(error.to_string().contains("data:[redacted]"));
        assert!(!error.to_string().contains("super-secret-source"));

        let warning = CompatibilityWarning::BackgroundThreadScript {
            url: xml_section_url(&input, "background-thread"),
        };
        assert_eq!(
            warning.to_string(),
            "a Lynx XML background-thread script at data:[redacted]#background-thread; background-thread JavaScript is retained but not executed"
        );
    }

    #[cfg(not(feature = "web-bundle"))]
    #[test]
    fn a_binary_web_bundle_requires_the_web_bundle_feature() {
        let error = PageSource::from_bytes(&input_url(), b"SDRA WROF")
            .expect_err("web-bundle support is disabled");

        assert!(matches!(error, SourceError::WebBundleSupportDisabled(_)));
    }

    #[test]
    fn native_bundle_magics_are_rejected_explicitly() {
        for magic in [0x0024_1922_u32, 0xdd73_7199] {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&8_u32.to_le_bytes());
            bytes.extend_from_slice(&magic.to_le_bytes());

            let error = PageSource::from_bytes(&input_url(), &bytes)
                .expect_err("native templates are a separate unsupported format");
            #[cfg(not(feature = "native-bundle"))]
            assert!(matches!(error, SourceError::UnsupportedNativeBundle(_)));
            #[cfg(feature = "native-bundle")]
            assert!(matches!(error, SourceError::DecodeNativeBundle { .. }));
        }
    }

    #[test]
    fn native_magic_with_a_wrong_total_size_still_stays_out_of_the_web_decoder() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&9_u32.to_le_bytes());
        bytes.extend_from_slice(&0x0024_1922_u32.to_le_bytes());

        let error = PageSource::from_bytes(&input_url(), &bytes)
            .expect_err("native decoding is a separate implementation");
        #[cfg(not(feature = "native-bundle"))]
        assert!(matches!(error, SourceError::UnsupportedNativeBundle(_)));
        #[cfg(feature = "native-bundle")]
        assert!(matches!(
            error,
            SourceError::DecodeNativeBundle {
                source: crate::native::ConvertError::SizeMismatch { .. },
                ..
            }
        ));
    }

    #[test]
    fn compatibility_warning_text_keeps_cli_wording_components() {
        let warning = CompatibilityWarning::ComponentScopedCss {
            css_ids: vec![4, 9],
        };
        assert_eq!(
            warning.to_string(),
            "component-scoped CSS fragments (css ids 4, 9); per-component scoping is not implemented, so their rules apply globally"
        );
    }

    #[test]
    fn background_warning_names_the_section_without_leaking_url_credentials() {
        let warning = CompatibilityWarning::BackgroundThreadScript {
            url: Url::parse(
                "https://user:secret@example.test/card.xml?token=secret#background-thread",
            )
            .expect("test URL"),
        };

        assert_eq!(
            warning.to_string(),
            "a Lynx XML background-thread script at /card.xml#background-thread; background-thread JavaScript is retained but not executed"
        );
    }
}
