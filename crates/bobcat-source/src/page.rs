//! Turns already-loaded Lynx page inputs into Bobcat view sources.
//!
//! [`PageSource::from_bytes`] sniffs and decodes a native or a web container
//! or strict UTF-8 Lynx XML for native embedders. [`register_lynx_xml_response`]
//! is the one-shot browser path: it accepts text decoded under browser
//! policy, maps sections to fragments of the final response URL, and
//! registers only the sources a view can load. Filesystem, network IO, and
//! byte-to-text policy remain the caller's responsibility.
//!
//! **A container page is a [`ContainerSources`].** Its registration path is
//! the one every container takes — the page's own, a `<frame>`'s, and one
//! `lynx.fetchBundle` fetched alike — so every body and stylesheet of it
//! hangs *under* the page's input URL `U` by the one URL rule
//! `crate::container` documents: the entry at `<U>/root.js`, a named chunk
//! or section at `<U>/<encoded name>.js`, a manifest path at `<U>/<path>`, a
//! named stylesheet at `<U>/custom_section/<encoded key>.css`, the page's
//! own `StyleInfo` at `<U>/style-info.css`, and the BTS boot script at
//! `U#background-thread`. The view names `U` as its card URL, so `__Card__`
//! is the container rather than its entry, and both realms build those same
//! URLs from it. A page adds only what makes it a page: it must have a
//! `root` (or, natively, the main-thread module named explicitly), and its
//! view lists the `StyleInfo` as its author sheet.
//!
//! Nothing here splices a source table into a script. Every body is
//! registered with the embedder's resource system at a URL of its own, and
//! what loads it is the realm, through the fetcher, on demand: BTS's
//! `lynx.requireModule`, `lynx.loadScript` and `nativeApp.loadScript` each
//! build the URL their path names and `require` it synchronously, and MTS's
//! `__LoadLepusChunk` loads a named chunk — a plain script resource,
//! registered verbatim — again on every call, as native's `TemplateEntry`
//! does. The boot script registers the container's own URL and starts the
//! card, and imports nothing.
//!
//! Every card body registered here is wrapped in a preamble, one physical
//! line long so the body keeps its own line numbering; only a column on the
//! body's first line is offset, by the preamble's length. This crate is what
//! wraps it, because the engine adds nothing to a script: it loads each as the
//! fetcher answered it. A body is wrapped in the list of names its realm
//! gives a card, which `bobcat-core` keeps:
//!
//! - An MTS body — the root Lepus script of a `.web.bundle` or a `.lynx.bundle`, and an XML page's
//!   main-thread script — is [`bobcat_core::MTS_CHUNK_PREAMBLE`] and then the body
//!   ([`mts_entry_source`]).
//! - A BTS body is [`bobcat_core::BTS_CHUNK_PREAMBLE`], which is every name web-core's chunk
//!   wrapper would have had as a parameter, plus — for a [`BundleTarget::Web`] body, which is a
//!   `CommonJS` file — a `module` object of its own and an `export default` of what it left there.
//!   A [`BundleTarget::Lynx`] body is one expression, whose value native's host keeps as a script
//!   completion value and which is `export default`'d here ([`bts_module_source`]). An XML page's
//!   background-thread script is a `Web` body, because web-core runs it through that same chunk
//!   wrapper.
//! - A container's `main-thread` or `*__main-thread` section is the same preamble with the body as
//!   the module's default export, because what `ReactLynx` calls is its value.
//! - Any other named Lepus chunk is wrapped in nothing at all: the realm compiles it as a function
//!   body, whose parameters are the bindings `MTS_CHUNK_PREAMBLE` gives a card's root, so an
//!   `import` could not appear in it anyway.
//!
//! The BTS boot script (`background_boot_source`) is no card body: it is
//! this crate's own module, and imports what it uses.

use std::sync::Arc;

use bobcat_core::{PageConfig, PreparsedStyleSheet, ScreenMetrics, ViewSources};
use bytes::Bytes;
use thiserror::Error;
use url::Url;
use vant::registry::{Register, RegisterInBackground};

use crate::ContainerSources;
use crate::container::Registrar;

#[derive(Clone, Debug)]
pub struct PageSource {
    input_url: Url,
    page: Page,
}

#[derive(Clone, Debug)]
enum Page {
    Container(ContainerSources),
    Xml(XmlPage),
}

#[derive(Clone, Debug)]
struct XmlPage {
    entry: (Url, Arc<str>),
    background: Option<(Url, Arc<str>)>,
    style: Option<(Url, Arc<str>)>,
}

#[derive(Debug)]
pub struct LynxXmlResponseRegistration {
    entry_url: Url,
    style_sheet_url: Option<Url>,
    background_thread_url: Option<Url>,
    compatibility_warnings: Vec<String>,
}

impl LynxXmlResponseRegistration {
    #[must_use]
    pub const fn entry_url(&self) -> &Url {
        &self.entry_url
    }

    #[must_use]
    pub const fn style_sheet_url(&self) -> Option<&Url> {
        self.style_sheet_url.as_ref()
    }

    #[must_use]
    pub const fn background_thread_url(&self) -> Option<&Url> {
        self.background_thread_url.as_ref()
    }

    #[must_use]
    pub fn compatibility_warnings(&self) -> &[String] {
        &self.compatibility_warnings
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SourceError {
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
    MissingRoot(String),
    #[error("could not decode native bundle `{input}`: {source}")]
    DecodeNativeBundle {
        input: String,
        #[source]
        source: crate::native::ConvertError,
    },
    #[error(
        "native bundle `{input}` has no main-thread module `{entry}`; select an external module explicitly"
    )]
    MissingNativeEntry { input: String, entry: String },
    #[error("`{0}` is a Lynx page rather than a container of sections")]
    NotAContainer(String),
    #[error("container `{input}` names `{url}` twice, with different contents")]
    ConflictingSection { input: String, url: String },
    #[error("container URL `{0}` cannot be a base for its sections")]
    OpaqueContainerUrl(String),
}

pub type PageSourceError = SourceError;

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
    main_thread: (Url, String),
    style: Option<(Url, &'source str)>,
    background_thread: Option<(Url, String)>,
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
    pub fn from_bytes(input: &Url, bytes: &[u8]) -> Result<Self, SourceError> {
        if looks_like_native_bundle(bytes) {
            Self::from_native_bundle(input, bytes, "root")
        } else if looks_like_lynx_xml(bytes) {
            let source = std::str::from_utf8(bytes).map_err(|source| {
                SourceError::InvalidLynxXmlEncoding {
                    input: diagnostic_url(input),
                    source,
                }
            })?;
            Self::from_lynx_xml(input, source)
        } else {
            Self::from_container(input, ContainerSources::from_bytes(input, bytes)?)
        }
    }

    pub fn from_native_bundle(input: &Url, bytes: &[u8], entry: &str) -> Result<Self, SourceError> {
        Self::from_container(
            input,
            ContainerSources::from_native_bundle(input, bytes, entry)?,
        )
    }

    fn from_container(input: &Url, container: ContainerSources) -> Result<Self, SourceError> {
        if container.entry().is_none() {
            return Err(SourceError::MissingRoot(diagnostic_url(input)));
        }
        Ok(Self {
            input_url: input.clone(),
            page: Page::Container(container),
        })
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the tests hand over the template they built"
    )]
    fn from_template_with_background(
        input: &Url,
        template: crate::web::WebTemplate,
        target: BundleTarget,
    ) -> Result<Self, SourceError> {
        Self::from_container(
            input,
            ContainerSources::from_template(input, &template, target, None)?,
        )
    }

    fn from_lynx_xml(input: &Url, source: &str) -> Result<Self, SourceError> {
        let mapped = map_lynx_xml(input, source, LynxXmlUrlPolicy::InMemory)?;
        let (entry_url, entry) = mapped.main_thread;
        Ok(Self {
            input_url: input.clone(),
            page: Page::Xml(XmlPage {
                entry: (entry_url, Arc::from(entry)),
                background: mapped
                    .background_thread
                    .map(|(url, source)| (url, Arc::from(source))),
                style: mapped.style.map(|(url, source)| (url, Arc::from(source))),
            }),
        })
    }

    #[must_use]
    pub const fn input_url(&self) -> &Url {
        &self.input_url
    }

    #[must_use]
    pub const fn config(&self) -> PageConfig {
        match &self.page {
            Page::Container(container) => container.config(),
            Page::Xml(_) => raw_lynx_xml_config(),
        }
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) const fn container(&self) -> Option<&ContainerSources> {
        match &self.page {
            Page::Container(container) => Some(container),
            Page::Xml(_) => None,
        }
    }

    pub fn register_with(&self, resources: &impl Register<PreparsedStyleSheet>) {
        match &self.page {
            Page::Container(container) => container.register_with(resources),
            Page::Xml(xml) => {
                let (url, entry) = &xml.entry;
                register_text(resources, url, entry, "text/javascript; charset=utf-8");
                if let Some((url, source)) = &xml.background {
                    register_text(resources, url, source, "text/javascript; charset=utf-8");
                }
                if let Some((url, style)) = &xml.style {
                    register_text(resources, url, style, "text/css; charset=utf-8");
                }
            }
        }
    }

    #[must_use]
    pub fn view_sources(&self, screen: ScreenMetrics, locale: &str) -> ViewSources {
        let input = self.input_url.to_string();
        match &self.page {
            Page::Container(container) => {
                let entry = container
                    .entry()
                    .expect("a container is a page only with an entry");
                ViewSources {
                    config: container.config(),
                    background_entry: container.background_entry().map(Url::to_string),
                    style_sheets: container
                        .style_info()
                        .map(Url::to_string)
                        .into_iter()
                        .collect(),
                    card_url: Some(container.url().to_string()),
                    ..ViewSources::new(input, entry.to_string(), screen, locale)
                }
            }
            Page::Xml(xml) => ViewSources {
                config: raw_lynx_xml_config(),
                background_entry: xml.background.as_ref().map(|(url, _)| url.to_string()),
                style_sheets: xml
                    .style
                    .as_ref()
                    .map(|(url, _)| url.to_string())
                    .into_iter()
                    .collect(),
                ..ViewSources::new(input, xml.entry.0.to_string(), screen, locale)
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeferredPage {
    input_url: Url,
    page: Deferred,
}

#[derive(Clone, Debug)]
enum Deferred {
    Decoded(Box<PageSource>),
    Web {
        bytes: Bytes,
        config: PageConfig,
        has_style_info: bool,
    },
}

impl PageSource {
    pub fn defer(input: &Url, bytes: impl Into<Bytes>) -> Result<DeferredPage, SourceError> {
        let bytes = bytes.into();
        let page = if looks_like_native_bundle(&bytes) || looks_like_lynx_xml(&bytes) {
            Deferred::Decoded(Box::new(Self::from_bytes(input, &bytes)?))
        } else {
            if input.cannot_be_a_base() {
                return Err(SourceError::OpaqueContainerUrl(diagnostic_url(input)));
            }
            let scan = crate::web::scan(&bytes).map_err(|source| SourceError::DecodeWebBundle {
                input: diagnostic_url(input),
                source,
            })?;
            if !scan.has_root {
                return Err(SourceError::MissingRoot(diagnostic_url(input)));
            }
            Deferred::Web {
                bytes,
                config: scan.config,
                has_style_info: scan.has_style_info,
            }
        };
        Ok(DeferredPage {
            input_url: input.clone(),
            page,
        })
    }
}

impl From<PageSource> for DeferredPage {
    fn from(page: PageSource) -> Self {
        Self {
            input_url: page.input_url.clone(),
            page: Deferred::Decoded(Box::new(page)),
        }
    }
}

impl DeferredPage {
    #[must_use]
    pub const fn input_url(&self) -> &Url {
        &self.input_url
    }

    #[must_use]
    pub const fn config(&self) -> PageConfig {
        match &self.page {
            Deferred::Decoded(page) => page.config(),
            Deferred::Web { config, .. } => *config,
        }
    }

    #[must_use]
    pub fn view_sources(&self, screen: ScreenMetrics, locale: &str) -> ViewSources {
        match &self.page {
            Deferred::Decoded(page) => page.view_sources(screen, locale),
            Deferred::Web {
                config,
                has_style_info,
                ..
            } => {
                let input = &self.input_url;
                ViewSources {
                    config: *config,
                    background_entry: Some(
                        crate::container::background_boot_url(input).to_string(),
                    ),
                    style_sheets: has_style_info
                        .then(|| crate::container::bundle_style_url(input).to_string())
                        .into_iter()
                        .collect(),
                    card_url: Some(input.to_string()),
                    ..ViewSources::new(
                        input.to_string(),
                        crate::container::named_chunk_url(input, crate::container::ROOT)
                            .to_string(),
                        screen,
                        locale,
                    )
                }
            }
        }
    }

    pub fn register_in_background(
        &self,
        resources: &impl RegisterInBackground<PreparsedStyleSheet>,
    ) {
        match &self.page {
            Deferred::Decoded(page) => page.register_with(resources),
            Deferred::Web {
                bytes,
                has_style_info,
                ..
            } => {
                let input = self.input_url.clone();
                let bytes = bytes.clone();
                let has_style_info = *has_style_info;
                resources.register_in_background(&self.input_url, move |registrar| {
                    register_web_page(&input, &bytes, has_style_info, registrar)
                });
            }
        }
    }
}

fn register_web_page(
    input: &Url,
    bytes: &[u8],
    listed_style_info: bool,
    registrar: &Registrar,
) -> Result<(), String> {
    let container =
        ContainerSources::from_bytes(input, bytes).map_err(|error| error.to_string())?;
    if container.entry().is_none() {
        return Err(SourceError::MissingRoot(diagnostic_url(input)).to_string());
    }
    let empty_sheet = (listed_style_info && container.style_info().is_none())
        .then(|| crate::container::bundle_style_url(input));
    if let Some(url) = &empty_sheet
        && container
            .scripts()
            .binary_search_by(|(script, _)| script.cmp(url))
            .is_ok()
    {
        return Err(SourceError::ConflictingSection {
            input: diagnostic_url(input),
            url: diagnostic_url(url),
        }
        .to_string());
    }
    container
        .register_through(registrar)
        .map_err(|error| error.to_string())?;
    if let Some(url) = empty_sheet {
        registrar
            .register_style_sheet(url.as_str(), Arc::new(PreparsedStyleSheet::default()))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleTarget {
    Lynx,
    Web,
}

#[must_use]
pub fn bts_module_source(target: BundleTarget, body: &str) -> String {
    const COMMONJS: &str = "const module = {exports: {}}; const exports = module.exports;";
    const EXPORT: &str = "\nexport default module.exports;";

    let mut source = String::with_capacity(
        bobcat_core::BTS_CHUNK_PREAMBLE.len() + COMMONJS.len() + body.len() + EXPORT.len(),
    );
    source.push_str(bobcat_core::BTS_CHUNK_PREAMBLE);
    match target {
        BundleTarget::Lynx => {
            source.push_str("export default ");
            source.push_str(body);
        }
        BundleTarget::Web => {
            source.push_str(COMMONJS);
            source.push_str(body);
            source.push_str(EXPORT);
        }
    }
    source
}

#[must_use]
pub fn mts_entry_source(body: &str) -> String {
    let mut source = String::with_capacity(bobcat_core::MTS_CHUNK_PREAMBLE.len() + body.len());
    source.push_str(bobcat_core::MTS_CHUNK_PREAMBLE);
    source.push_str(body);
    source
}

fn xml_background_source(body: &str) -> String {
    bts_module_source(BundleTarget::Web, body)
}

pub(crate) fn background_boot_source(input: &Url, has_app_service: bool) -> String {
    let template_url = serde_json::to_string(input.as_str()).expect("a URL is a JavaScript string");
    let mut source = format!(
        "import {{lynx, __BobcatRegisterBundle}} from 'bobcat:bts-runtime';\n\
         __BobcatRegisterBundle({template_url});\n"
    );
    if has_app_service {
        source.push_str("lynx.requireModule('/app-service.js');\n");
    }
    source
}

pub fn register_lynx_xml_response(
    input: &Url,
    source: &str,
    resources: &impl Register<PreparsedStyleSheet>,
) -> Result<LynxXmlResponseRegistration, SourceError> {
    let mapped = map_lynx_xml(input, source, LynxXmlUrlPolicy::ResponseFragments)?;
    let background_thread_url = mapped.background_thread.map(|(url, source)| {
        register_text(resources, &url, &source, "text/javascript; charset=utf-8");
        url
    });
    let (entry_url, main_thread_script) = mapped.main_thread;
    register_text(
        resources,
        &entry_url,
        &main_thread_script,
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
        compatibility_warnings: Vec::new(),
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
        main_thread: (main_thread, mts_entry_source(xml.main_thread_script)),
        style: xml.style.map(|source| (style, source)),
        background_thread: xml
            .background_thread_script
            .map(|source| (background_thread, xml_background_source(source))),
    })
}

const fn raw_lynx_xml_config() -> PageConfig {
    PageConfig {
        default_display_linear: false,
        default_overflow_visible: false,
        enable_css_selector: true,
        enable_js_data_processor: false,
        enable_css_inheritance: false,
    }
}

fn register_text(
    resources: &impl Register<PreparsedStyleSheet>,
    url: &Url,
    source: &str,
    media_type: &str,
) {
    resources
        .register(
            url.as_str(),
            Bytes::copy_from_slice(source.as_bytes()),
            Some(media_type),
        )
        .expect("PageSource's registration URLs are valid");
}

fn xml_section_url(input: &Url, fragment: &str) -> Url {
    let mut url = input.clone();
    url.set_fragment(Some(fragment));
    url
}

pub(crate) fn diagnostic_url(input: &Url) -> String {
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

pub(crate) fn bounded_diagnostic(mut value: String) -> String {
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

pub(crate) fn looks_like_native_bundle(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..8) else {
        return false;
    };
    let magic = u32::from_le_bytes(header[4..].try_into().expect("four-byte native magic"));
    matches!(magic, 0x0024_1922 | 0xdd73_7199)
}

pub(crate) fn looks_like_web_bundle(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..8) else {
        return false;
    };
    let magic0 = u32::from_le_bytes(header[..4].try_into().expect("four-byte web magic"));
    let magic1 = u32::from_le_bytes(header[4..].try_into().expect("four-byte web magic"));
    magic0 == crate::web::MAGIC_0 && magic1 == crate::web::MAGIC_1
}

pub(crate) fn looks_like_lynx_xml(mut bytes: &[u8]) -> bool {
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::collections::BTreeMap;

    use bobcat_resources::{Resources, ResourcesConfig};

    use super::*;

    fn input_url() -> Url {
        Url::parse("file:///tmp/card.lynx.xml").expect("test URL")
    }

    fn bundle_url() -> Url {
        Url::parse("file:///tmp/card.web.bundle").expect("test URL")
    }

    fn script_at<'a>(page: &'a PageSource, url: &str) -> &'a str {
        match &page.page {
            Page::Container(container) => container
                .scripts()
                .iter()
                .find(|(at, _)| at.as_str() == url)
                .map_or_else(|| panic!("no script at {url}"), |(_, text)| text.as_ref()),
            Page::Xml(xml) => [Some(&xml.entry), xml.background.as_ref()]
                .into_iter()
                .flatten()
                .find(|(at, _)| at.as_str() == url)
                .map_or_else(|| panic!("no script at {url}"), |(_, text)| text.as_ref()),
        }
    }

    fn entry_script(page: &PageSource) -> &str {
        script_at(page, &page.view_sources(SCREEN, "en-US").entry)
    }

    fn background_script(page: &PageSource) -> Option<&str> {
        page.view_sources(SCREEN, "en-US")
            .background_entry
            .map(|url| script_at(page, &url))
    }

    fn xml(page: &PageSource) -> &XmlPage {
        match &page.page {
            Page::Xml(xml) => xml,
            Page::Container(_) => panic!("a container page"),
        }
    }

    const SCREEN: ScreenMetrics = ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

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

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_section(bytes: &mut Vec<u8>, label: u32, content: &[u8]) {
        push_u32(bytes, label);
        push_u32(
            bytes,
            u32::try_from(content.len()).expect("tiny test section"),
        );
        bytes.extend_from_slice(content);
    }

    fn push_string(bytes: &mut Vec<u8>, value: &str) {
        push_u32(bytes, u32::try_from(value.len()).expect("tiny test string"));
        bytes.extend_from_slice(value.as_bytes());
    }

    fn web_bundle(root: Option<&str>) -> Vec<u8> {
        web_bundle_with_config(
            r#"{
            "defaultDisplayLinear": "true",
            "defaultOverflowVisible": "false",
            "enableCSSSelector": "true"
        }"#,
            root,
        )
    }

    fn web_bundle_with_config(config: &str, root: Option<&str>) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, crate::web::MAGIC_0);
        push_u32(&mut bytes, crate::web::MAGIC_1);
        push_u32(&mut bytes, 1);

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
                enable_js_data_processor: false,
                enable_css_inheritance: false,
            }
        );
        let sources = page.view_sources(SCREEN, "en-US");
        assert_eq!(sources.base_url, page.input_url().as_str());
        assert_eq!(sources.config, page.config());
        assert_eq!(
            sources.entry,
            "https://example.test/card.web.bundle/root.js"
        );
        assert_eq!(
            sources.card_url.as_deref(),
            Some("https://example.test/card.web.bundle"),
            "`__Card__` is the container, not its entry"
        );
        assert_eq!(
            sources.background_entry.as_deref(),
            Some("https://example.test/card.web.bundle#background-thread")
        );
        assert_eq!(sources.style_sheets, [] as [String; 0]);

        let resources = resources();
        page.register_with(&resources);
        assert!(resources.unregister(&sources.entry));
    }

    #[test]
    fn the_bts_boot_script_registers_the_bundle_under_the_inputs_own_url() {
        let input = Url::parse("https://cdn.example/app/card.web.bundle").expect("test URL");
        let mut template = crate::web::decode(&web_bundle(Some("export {};"))).unwrap();
        template
            .manifest
            .insert("/app-service.js".into(), "module.exports = {};".into());
        template.custom_sections =
            Some(serde_json::json!({"answer": {"content": "module.exports = 42"}}));
        let page = PageSource::from_template_with_background(&input, template, BundleTarget::Web)
            .expect("a manifest page");

        let sources = page.view_sources(SCREEN, "en-US");
        assert_eq!(
            sources.background_entry.as_deref(),
            Some("https://cdn.example/app/card.web.bundle#background-thread")
        );
        assert_eq!(
            background_script(&page).expect("a BTS boot script"),
            concat!(
                "import {lynx, __BobcatRegisterBundle} from 'bobcat:bts-runtime';\n",
                "__BobcatRegisterBundle(\"https://cdn.example/app/card.web.bundle\");\n",
                "lynx.requireModule('/app-service.js');\n",
            ),
            "the boot script carries the container's URL and nothing of its bodies"
        );
    }

    #[test]
    fn manifest_paths_and_string_custom_sections_register_under_the_input_url() {
        let input = Url::parse("https://cdn.example/app/card.web.bundle").expect("test URL");
        let mut template = crate::web::decode(&web_bundle(Some("export {};"))).unwrap();
        template
            .manifest
            .insert("/app-service.js".into(), "module.exports = {};".into());
        template.custom_sections = Some(serde_json::json!({
            "answer": {"content": "module.exports = 42"},
            "binary": {"content": [1, 2, 3]},
        }));
        let page = PageSource::from_template_with_background(&input, template, BundleTarget::Web)
            .expect("a manifest page");

        let registered: Vec<&str> = page
            .container()
            .expect("a container page")
            .scripts()
            .iter()
            .map(|(url, _)| url.as_str())
            .collect();
        assert_eq!(
            registered,
            [
                "https://cdn.example/app/card.web.bundle#background-thread",
                "https://cdn.example/app/card.web.bundle/answer.js",
                "https://cdn.example/app/card.web.bundle/app-service.js",
                "https://cdn.example/app/card.web.bundle/root.js",
            ],
            "a section whose content is not a string carries no body to register"
        );
        assert_eq!(
            script_at(&page, "https://cdn.example/app/card.web.bundle/answer.js"),
            bts_module_source(BundleTarget::Web, "module.exports = 42")
        );
        assert_eq!(
            script_at(
                &page,
                "https://cdn.example/app/card.web.bundle/app-service.js"
            ),
            bts_module_source(BundleTarget::Web, "module.exports = {};")
        );

        let resources = resources();
        page.register_with(&resources);
        assert!(resources.unregister("https://cdn.example/app/card.web.bundle/app-service.js"));
        assert!(resources.unregister("https://cdn.example/app/card.web.bundle/answer.js"));
    }

    #[test]
    fn a_web_body_becomes_a_commonjs_module_around_its_own_line_one() {
        let module = bts_module_source(BundleTarget::Web, "module.exports = 21 * 2;");
        assert_eq!(
            module,
            format!(
                "{}const module = {{exports: {{}}}}; const exports = module.exports;\
                 module.exports = 21 * 2;\nexport default module.exports;",
                bobcat_core::BTS_CHUNK_PREAMBLE
            )
        );
        assert_eq!(
            module.lines().count(),
            2,
            "one line for the body, one for the export it answers through"
        );
    }

    #[test]
    fn a_lynx_body_becomes_the_default_export_of_its_own_expression() {
        assert_eq!(
            bts_module_source(
                BundleTarget::Lynx,
                "(function(){'use strict';return{init:n}})()"
            ),
            format!(
                "{}export default (function(){{'use strict';return{{init:n}}}})()",
                bobcat_core::BTS_CHUNK_PREAMBLE
            )
        );
        assert_eq!(
            bts_module_source(
                BundleTarget::Lynx,
                "(function(){})();\n//# sourceMappingURL=background.js.map"
            ),
            format!(
                "{}export default (function(){{}})();\n\
                 //# sourceMappingURL=background.js.map",
                bobcat_core::BTS_CHUNK_PREAMBLE
            )
        );
    }

    #[test]
    fn a_bodys_first_line_stays_its_first_line() {
        for target in [BundleTarget::Web, BundleTarget::Lynx] {
            let module = bts_module_source(target, "first();\nsecond();\nthird();");
            let lines: Vec<&str> = module.lines().collect();
            assert!(lines[0].ends_with("first();"), "{target:?}: {}", lines[0]);
            assert_eq!(lines[1], "second();", "{target:?}");
            assert_eq!(lines[2], "third();", "{target:?}");
        }
    }

    #[test]
    fn a_json_body_is_registered_as_the_json_it_is() {
        for target in [BundleTarget::Web, BundleTarget::Lynx] {
            let mut template = crate::web::decode(&web_bundle(Some("export {};"))).unwrap();
            template
                .manifest
                .insert("/data.json".into(), r#"{"message": "hi"}"#.into());
            let page = PageSource::from_template_with_background(&bundle_url(), template, target)
                .expect("a manifest page");
            assert_eq!(
                script_at(&page, "file:///tmp/card.web.bundle/data.json"),
                r#"{"message": "hi"}"#,
                "{target:?}"
            );
        }
    }

    #[test]
    fn a_main_thread_section_is_no_bts_body() {
        let mut template = crate::web::decode(&web_bundle(Some("export {};"))).unwrap();
        template
            .manifest
            .insert("/app-service.js".into(), "module.exports = {};".into());
        template.custom_sections = Some(serde_json::json!({
            "card__main-thread": {"content": "const a = 1; renderPage();"},
            "answer": {"content": "module.exports = 42"},
        }));
        let page =
            PageSource::from_template_with_background(&bundle_url(), template, BundleTarget::Web)
                .expect("a manifest page");

        let section = script_at(&page, "file:///tmp/card.web.bundle/card__main-thread.js");
        assert!(section.starts_with(bobcat_core::MTS_CHUNK_PREAMBLE));
        assert!(
            section.ends_with("export default const a = 1; renderPage();"),
            "{section}"
        );
        assert!(!section.starts_with(bobcat_core::BTS_CHUNK_PREAMBLE));
        let boot = background_script(&page).expect("a BTS boot script");
        assert!(
            !boot.contains("main-thread"),
            "no section is named in the boot script at all: {boot}"
        );
    }

    const THREE_LINES: &str = "first();\nsecond();\nthird();";

    fn assert_mts_entry_of_three_lines(source: &str, what: &str) {
        assert_eq!(
            source,
            format!("{}{THREE_LINES}", bobcat_core::MTS_CHUNK_PREAMBLE),
            "{what}"
        );
        let lines: Vec<&str> = source.lines().collect();
        assert_eq!(
            lines,
            [
                format!("{}first();", bobcat_core::MTS_CHUNK_PREAMBLE).as_str(),
                "second();",
                "third();"
            ],
            "{what}: every line of the body keeps its number"
        );
    }

    #[test]
    fn a_bundle_root_is_the_mts_preamble_and_its_body_from_line_one() {
        let web = PageSource::from_bytes(&bundle_url(), &web_bundle(Some(THREE_LINES)))
            .expect("a web bundle");
        assert_mts_entry_of_three_lines(entry_script(&web), "web bundle");

        let template = crate::web::decode(&web_bundle(Some(THREE_LINES))).unwrap();
        let native =
            PageSource::from_template_with_background(&bundle_url(), template, BundleTarget::Lynx)
                .expect("a native bundle's template");
        assert_mts_entry_of_three_lines(entry_script(&native), "native bundle");
    }

    #[test]
    fn both_xml_paths_wrap_both_scripts() {
        let xml = format!(
            "<lynx engine-version=\"4.2\"><script thread=\"main\">{THREE_LINES}</script>\
             <script thread=\"background\">lynx.getCoreContext();</script></lynx>"
        );
        let page = PageSource::from_bytes(&input_url(), xml.as_bytes()).expect("an XML page");
        assert_mts_entry_of_three_lines(entry_script(&page), "XML page");
        assert_eq!(
            background_script(&page),
            Some(bts_module_source(BundleTarget::Web, "lynx.getCoreContext();").as_str())
        );

        let mapped = map_lynx_xml(&input_url(), &xml, LynxXmlUrlPolicy::ResponseFragments)
            .expect("an XML response");
        assert_eq!(
            mapped.main_thread.0.as_str(),
            "file:///tmp/card.lynx.xml#main-thread"
        );
        assert_mts_entry_of_three_lines(&mapped.main_thread.1, "XML response");
        assert_eq!(
            mapped.background_thread.map(|(_, source)| source),
            Some(bts_module_source(
                BundleTarget::Web,
                "lynx.getCoreContext();"
            ))
        );
    }

    #[test]
    fn non_root_lepus_chunks_register_under_the_container() {
        let input = Url::parse("https://example.test/card.web.bundle").expect("test URL");
        let mut template = crate::web::decode(&web_bundle(Some("export {};"))).unwrap();
        template
            .lepus_code
            .insert("A &/中".into(), "globalThis.chunkRan = true;".into());
        let page = PageSource::from_template_with_background(&input, template, BundleTarget::Web)
            .expect("a page with a named chunk");

        assert_eq!(
            script_at(
                &page,
                "https://example.test/card.web.bundle/A%20%26%2F%E4%B8%AD.js"
            ),
            "globalThis.chunkRan = true;",
            "a chunk is registered verbatim: no preamble, nothing to import"
        );
        assert_eq!(entry_script(&page), mts_entry_source("export {};"));

        let resources = resources();
        page.register_with(&resources);
        assert!(
            resources.unregister("https://example.test/card.web.bundle/A%20%26%2F%E4%B8%AD.js")
        );
    }

    async fn binding_painter() -> bobcat_core::Painter {
        bobcat_core::Painter::new(bobcat_core::DrawTarget::Offscreen, 32.0, 24.0, 1.0)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn registered_compiler_sources_boot_bts_for_web_and_native_wrappers() {
        use std::time::{Duration, Instant};

        use bobcat_core::{EngineEvent, LynxGroup, NoWakeup, StyleThreads};

        const CARD: &str = r#"
            app.define('entry.js', function(require,module,exports,Card,setTimeout,setInterval,clearInterval,clearTimeout,NativeModules,api,console,Component,ReactLynx,nativeAppId,Behavior,LynxJSBI,lynx) {
                if (api !== app._apiList) throw Error('factory ABI');
                if (lynx.loadScript('answer', {}) !== 42) throw Error('named section');
                if (lynx.loadScript('/answer', {}) !== 42) throw Error('rooted section name');
                if (lynx.requireModule('/data.json').message !== `quotes ' and " `) throw Error('JSON source');
                if (globalThis.globDynamicComponentEntry !== '__Card__') throw Error('entry while initializing');
                console.log('compiler bootstrap ready');
                module.exports = {value:42};
            });
            if (app.require('entry.js').value !== 42) throw Error('module exports');
        "#;

        const SHAPES: &str = r"
            for (const shape of ['compact', 'minified', 'pretty'])
                if (lynx.loadScript(shape, {}) !== shape) throw Error('banner ' + shape);
        ";

        for target in [BundleTarget::Web, BundleTarget::Lynx] {
            let mut template = crate::web::decode(&web_bundle(Some("export {};"))).unwrap();
            template.manifest.insert(
                "/app-service.js".into(),
                match target {
                    BundleTarget::Web => format!(
                        "'use strict';\nconst app = lynxCoreInject.tt;\n\
                         module.exports = {{init({{tt}}) {{\
                         if (tt !== app) throw Error('injection');{CARD}}}}};"
                    ),
                    BundleTarget::Lynx => format!(
                        "(function(){{'use strict';return {{init({{tt}}){{\
                         const app = tt;{CARD}{SHAPES}}}}};}})();\n\
                         //# sourceMappingURL=app-service.js.map"
                    ),
                },
            );
            template.manifest.insert(
                "/data.json".into(),
                serde_json::json!({"message": "quotes ' and \" "}).to_string(),
            );
            let mut sections = serde_json::json!({
                "answer": {"content": match target {
                    BundleTarget::Web => "module.exports = 21 * 2",
                    BundleTarget::Lynx => "21 * 2",
                }},
            });
            if matches!(target, BundleTarget::Lynx) {
                let shapes = serde_json::json!({
                    "compact": {"content":
                        "(function(){'use strict';return{init:function(){return 'compact'}}})()"},
                    "minified": {"content":
                        "(function(){\"use strict\";var e=globalThis;\
                         return{init:function(){return 'minified'}}})();\n\
                         //# sourceMappingURL=/.lynx/card/background.js.map"},
                    "pretty": {"content":
                        "(function(){\n  'use strict';\n  var g = globalThis;\n  \
                         return {init: function () { return 'pretty'; }};\n})();\n\n\
                         //# sourceMappingURL=background.js.map\n"},
                });
                let (sections, shapes) = (
                    sections.as_object_mut().expect("an object"),
                    shapes.as_object().expect("an object"),
                );
                sections.extend(shapes.clone());
            }
            template.custom_sections = Some(sections);
            let page =
                PageSource::from_template_with_background(&input_url(), template, target).unwrap();
            let resources = resources();
            page.register_with(&resources);
            let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
                .await
                .unwrap();
            let mut view = group
                .create_lynx_view(
                    32.0,
                    24.0,
                    1.0,
                    resources.builder(),
                    Vec::new(),
                    page.view_sources(SCREEN, "en-US"),
                )
                .unwrap();
            let mut painter = binding_painter().await;
            painter.attach(&view).unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut reported = false;
            while !view.is_ready() || !reported {
                for event in view.pump() {
                    match event {
                        EngineEvent::ScriptFinished => {}
                        EngineEvent::ConsoleMessage { message, .. } => {
                            assert_eq!(message, "compiler bootstrap ready");
                            reported = true;
                        }
                        event => panic!("compiler bootstrap failed: {event:?}"),
                    }
                }
                assert!(Instant::now() < deadline, "BTS did not become ready");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    }

    #[test]
    fn a_page_registers_what_a_fetch_of_its_bytes_registers() {
        for name in [
            "basic-class-selector",
            "css-custom-sections",
            "react-bts-query",
        ] {
            let input =
                Url::parse(&format!("https://cdn.test/app/{name}.web.bundle?rev=1")).unwrap();
            let bytes = crate::fixtures::fixture(name).page;
            let page = PageSource::from_bytes(&input, bytes).expect("the card decodes");
            let fetched = ContainerSources::from_bytes(&input, bytes).expect("the card decodes");
            assert_eq!(page.container(), Some(&fetched), "{name}");
            assert!(fetched.entry().is_some(), "{name} has a root");
        }
    }

    #[test]
    fn a_native_page_is_its_fetch_plus_the_named_entry_and_its_boot() {
        let input = Url::parse("https://cdn.test/app/react-native.lynx.bundle").unwrap();
        let bytes = crate::fixtures::fixture("react-native").page;
        let entry = "react-native__main-thread";
        let page = PageSource::from_native_bundle(&input, bytes, entry).expect("the card decodes");
        let page = page.container().expect("a container page");
        let fetched = ContainerSources::from_bytes(&input, bytes).expect("the card decodes");
        assert!(fetched.entry().is_none() && fetched.background_entry().is_none());

        let template = crate::native::decode(bytes).unwrap();
        let mut expected: BTreeMap<String, String> = fetched
            .scripts()
            .iter()
            .map(|(url, text)| (url.to_string(), text.to_string()))
            .collect();
        expected.insert(
            format!("{input}/root.js"),
            mts_entry_source(&template.lepus_code[entry]),
        );
        expected.insert(
            format!("{input}#background-thread"),
            background_boot_source(&input, template.manifest.contains_key("/app-service.js")),
        );
        let registered: BTreeMap<String, String> = page
            .scripts()
            .iter()
            .map(|(url, text)| (url.to_string(), text.to_string()))
            .collect();
        assert_eq!(registered, expected);
        assert_eq!(page.style_sheets(), fetched.style_sheets());
        assert_eq!(page.style_info(), fetched.style_info());
        assert_eq!(
            page.entry().map(Url::as_str),
            Some(format!("{input}/root.js").as_str())
        );
        assert_eq!(
            page.background_entry().map(Url::as_str),
            Some(format!("{input}#background-thread").as_str())
        );
    }

    #[tokio::test]
    async fn a_container_page_boots_with_the_container_as_its_card() {
        use std::time::{Duration, Instant};

        use bobcat_core::{EngineEvent, LynxGroup, NoWakeup, StyleThreads};

        let input = Url::parse("https://cdn.test/app/card.web.bundle?rev=1").unwrap();
        let mut template = crate::web::decode(&web_bundle(Some(
            r"
            if (__Card__ !== 'https://cdn.test/app/card.web.bundle?rev=1')
                throw Error('the card is not the container: ' + __Card__);
            if (__LoadLepusChunk('chunk', {}) !== true) throw Error('the chunk was not found');
            if (globalThis.chunkRuns !== 1) throw Error('the chunk did not run');
            if (__LoadStyleSheet('CSS', '__Card__') === null)
                throw Error('the CSS section is not held');
            console.log('container card checked');
        ",
        )))
        .unwrap();
        template.lepus_code.insert(
            "chunk".into(),
            "globalThis.chunkRuns = (globalThis.chunkRuns ?? 0) + 1;".into(),
        );
        template.custom_sections = Some(serde_json::json!({
            "CSS": {"encoding": "CSS", "content": {"ruleList": [
                {"type": "StyleRule", "selectorText": {"value": ".box"},
                 "style": [{"name": "width", "value": "10px"}]}
            ]}},
        }));
        let page =
            PageSource::from_template_with_background(&input, template, BundleTarget::Web).unwrap();
        let sources = page.view_sources(SCREEN, "en-US");
        assert_eq!(
            sources.entry,
            "https://cdn.test/app/card.web.bundle/root.js?rev=1"
        );
        let resources = resources();
        page.register_with(&resources);
        let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
            .await
            .unwrap();
        let mut view = group
            .create_lynx_view(32.0, 24.0, 1.0, resources.builder(), Vec::new(), sources)
            .unwrap();
        let mut painter = binding_painter().await;
        painter.attach(&view).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let (mut checked, mut finished) = (false, false);
        while !checked || !finished {
            for event in view.pump() {
                match event {
                    EngineEvent::ScriptFinished => finished = true,
                    EngineEvent::ConsoleMessage { message, .. } => {
                        assert_eq!(message, "container card checked");
                        checked = true;
                    }
                    event => panic!("the container card failed: {event:?}"),
                }
            }
            assert!(
                Instant::now() < deadline,
                "checked={checked} finished={finished}"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    #[test]
    fn web_bundle_css_inheritance_follows_the_page_config_string() {
        for (config, expected) in [
            (r#"{"enableCSSInheritance": "true"}"#, true),
            (r#"{"enableCSSInheritance": "false"}"#, false),
            (r#"{"enableCSSInheritance": "TRUE"}"#, false),
            (r#"{"enableCSSInheritance": true}"#, false),
            (r#"{"enableCssInheritance": "true"}"#, false),
            ("{}", false),
        ] {
            let page = PageSource::from_bytes(
                &input_url(),
                &web_bundle_with_config(config, Some("export {};")),
            )
            .expect("valid web bundle");

            assert_eq!(page.config().enable_css_inheritance, expected, "{config}");
            assert_eq!(
                page.view_sources(SCREEN, "en-US")
                    .config
                    .enable_css_inheritance,
                expected,
                "the view is built with the page's switch: {config}"
            );
        }
    }

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
        assert!(!page.config().enable_css_inheritance);
        assert_eq!(page.input_url(), &input_url());
        assert_eq!(
            xml(&page).style.as_ref().map(|(url, _)| url.as_str()),
            Some("bobcat-memory://lynx-xml/style.css")
        );
        assert!(matches!(
            xml(&page).style.as_ref(),
            Some((_, source)) if source.is_empty()
        ));
        assert_eq!(
            background_script(&page),
            Some(bts_module_source(BundleTarget::Web, "").as_str())
        );
        assert!(page.view_sources(SCREEN, "en-US").card_url.is_none());
        let sources = page.view_sources(SCREEN, "en-US");
        assert_eq!(sources.base_url, page.input_url().as_str());
        assert_eq!(sources.config, page.config());
        assert_eq!(sources.entry, "bobcat-memory://lynx-xml/main-thread.js");
        assert_eq!(
            sources.background_entry.as_deref(),
            Some("bobcat-memory://lynx-xml/app-service.js")
        );
        assert_eq!(
            sources.style_sheets,
            vec!["bobcat-memory://lynx-xml/style.css".to_owned()]
        );

        let resources = resources();
        page.register_with(&resources);
        assert!(resources.unregister(&sources.entry));
        assert!(resources.unregister(sources.background_entry.as_deref().unwrap()));
        assert!(resources.unregister(&sources.style_sheets[0]));
    }

    #[test]
    fn browser_response_registers_both_script_realms_at_final_response_fragments() {
        let input = Url::parse("https://cdn.example/final/card.lynx.xml?revision=2#request")
            .expect("test URL");
        let resources = resources();
        let registered = register_lynx_xml_response(
            &input,
            "<lynx engine-version=\"4.2\"><style></style><script thread=\"main\">main</script><script thread=\"background\">lynx.getCoreContext();</script></lynx>",
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
        assert_eq!(registered.compatibility_warnings(), [] as [String; 0]);
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
            resources.unregister(
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

        assert!(xml(&page).style.is_none());
        assert!(xml(&page).background.is_none());
        assert!(
            page.view_sources(SCREEN, "en-US")
                .background_entry
                .is_none()
        );
        assert_eq!(
            page.view_sources(SCREEN, "en-US").entry,
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
    fn opaque_urls_are_redacted_in_errors() {
        let input = Url::parse("data:text/xml,super-secret-source").expect("test data URL");
        let error = register_lynx_xml_response(&input, "<lynx", &resources())
            .expect_err("malformed XML must fail");
        assert!(error.to_string().contains("data:[redacted]"));
        assert!(!error.to_string().contains("super-secret-source"));
    }

    #[test]
    fn truncated_native_bundles_report_native_decode_errors() {
        for magic in [0x0024_1922_u32, 0xdd73_7199] {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&8_u32.to_le_bytes());
            bytes.extend_from_slice(&magic.to_le_bytes());

            let error = PageSource::from_bytes(&input_url(), &bytes)
                .expect_err("native header without sections is truncated");
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
        assert!(matches!(
            error,
            SourceError::DecodeNativeBundle {
                source: crate::native::ConvertError::SizeMismatch { .. },
                ..
            }
        ));
    }

    #[test]
    fn a_deferred_page_names_the_sources_its_decode_names() {
        let screen = ScreenMetrics::for_viewport(32.0, 24.0, 1.0);
        let input = Url::parse("https://cdn.test/app/card.web.bundle?rev=1").unwrap();
        let bundles = crate::compiled_web_bundles();
        assert!(bundles.len() > 300, "build the fixture packages first");
        for (path, bytes) in bundles {
            let Ok(page) = PageSource::from_bytes(&input, &bytes) else {
                assert!(
                    matches!(
                        PageSource::defer(&input, bytes),
                        Err(SourceError::MissingRoot(_))
                    ),
                    "{path}"
                );
                continue;
            };
            let deferred = PageSource::defer(&input, bytes).expect("a page defers");
            let mut expected = page.view_sources(screen, "en-US");
            let empty_style_info = page.container().unwrap().style_info().is_none();
            if empty_style_info
                && !deferred
                    .view_sources(screen, "en-US")
                    .style_sheets
                    .is_empty()
            {
                expected.style_sheets =
                    vec!["https://cdn.test/app/card.web.bundle/style-info.css?rev=1".to_string()];
            }
            assert_eq!(
                format!("{:?}", deferred.view_sources(screen, "en-US")),
                format!("{expected:?}"),
                "{path}"
            );
            assert_eq!(deferred.config(), page.config(), "{path}");
        }
    }

    #[test]
    fn a_deferred_page_refuses_what_its_scan_can_tell() {
        let input = input_url();
        assert!(matches!(
            PageSource::defer(&input, web_bundle(None)),
            Err(SourceError::MissingRoot(_))
        ));
        assert!(matches!(
            PageSource::defer(
                &Url::parse("data:text/plain,card").unwrap(),
                web_bundle(Some("x"))
            ),
            Err(SourceError::OpaqueContainerUrl(_))
        ));
        let mut unknown = web_bundle(Some("x"));
        push_section(&mut unknown, 99, b"");
        assert!(matches!(
            PageSource::defer(&input, unknown),
            Err(SourceError::DecodeWebBundle {
                source: crate::web::DecodeError::UnknownSection(99),
                ..
            })
        ));
        let mut truncated = web_bundle(Some("x"));
        truncated.truncate(truncated.len() - 1);
        assert!(PageSource::defer(&input, truncated).is_err());
        assert!(matches!(
            PageSource::defer(&input, web_bundle_with_config("[]", Some("x"))),
            Err(SourceError::DecodeWebBundle { .. })
        ));
    }

    fn eventually(mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "the background install finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn quiet_resources() -> Resources {
        Resources::new(
            bobcat_resources::ResourcesConfig {
                log_to_stderr: false,
                ..bobcat_resources::ResourcesConfig::default()
            },
            || {},
        )
    }

    #[test]
    fn a_deferred_page_registers_its_container_in_the_background() {
        let input = Url::parse("https://cdn.test/app/basic-bindtap.web.bundle").unwrap();
        let bytes = crate::fixtures::fixture("basic-bindtap").page;
        let decoded = PageSource::from_bytes(&input, bytes).unwrap();
        let container = decoded.container().unwrap();
        assert!(
            container.style_info().is_none(),
            "the card's StyleInfo is empty"
        );
        let deferred = PageSource::defer(&input, bytes.to_vec()).unwrap();
        let screen = ScreenMetrics::for_viewport(32.0, 24.0, 1.0);
        let sheet = "https://cdn.test/app/basic-bindtap.web.bundle/style-info.css";
        assert_eq!(deferred.view_sources(screen, "en-US").style_sheets, [sheet]);

        let resources = quiet_resources();
        deferred.register_in_background(&resources);
        eventually(|| resources.unregister(sheet));
        for (url, _) in container.scripts() {
            assert!(resources.unregister(url.as_str()), "{url}");
        }
        assert_eq!(resources.take_notes(), [] as [String; 0]);
    }

    #[test]
    fn a_deferred_page_that_fails_to_decode_registers_nothing() {
        let input = input_url();
        let mut bytes = web_bundle(Some("export {};"));
        push_section(
            &mut bytes,
            crate::web::SectionLabel::StyleInfo as u32,
            &[0xFF; 64],
        );
        let deferred = PageSource::defer(&input, bytes).expect("the scan reads no StyleInfo");
        let resources = quiet_resources();
        deferred.register_in_background(&resources);
        let mut notes = Vec::new();
        eventually(|| {
            notes.extend(resources.take_notes());
            !notes.is_empty()
        });
        assert!(notes[0].contains("invalid StyleInfo section"), "{notes:?}");
        let entry = deferred
            .view_sources(ScreenMetrics::for_viewport(1.0, 1.0, 1.0), "en-US")
            .entry;
        assert!(!resources.unregister(&entry));
    }
}
