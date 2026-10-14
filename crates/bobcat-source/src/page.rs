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
//! - A named Lepus chunk is wrapped in nothing at all: the realm compiles it as a function body,
//!   whose parameters are the bindings `MTS_CHUNK_PREAMBLE` gives a card's root, so an `import`
//!   could not appear in it anyway.
//!
//! The BTS boot script ([`background_boot_source`]) is no card body: it is
//! this crate's own module, and imports what it uses.

use std::sync::Arc;

use bobcat_core::{PageConfig, ScreenMetrics, ViewSources};
use bobcat_resources::Resources;
use thiserror::Error;
use url::Url;

use crate::ContainerSources;

/// A decoded page and the in-memory resources it contributes to a Bobcat
/// view.
#[derive(Clone, Debug)]
pub struct PageSource {
    input_url: Url,
    page: Page,
}

#[derive(Clone, Debug)]
enum Page {
    /// A native or web container with an entry, registered under its input
    /// URL by the one container rule.
    Container(ContainerSources),
    /// A raw Lynx XML page, at private memory URLs.
    Xml(XmlPage),
}

/// A raw Lynx XML page's sections, each as the text registered for it.
#[derive(Clone, Debug)]
struct XmlPage {
    /// The main-thread script, as the module [`mts_entry_source`] writes.
    entry: (Url, Arc<str>),
    /// The background-thread script, as a [`BundleTarget::Web`] body.
    background: Option<(Url, Arc<str>)>,
    /// The `<style>` section's CSS, verbatim.
    style: Option<(Url, Arc<str>)>,
}

/// The view-facing result of registering one browser-loaded Lynx XML response.
///
/// Both script bodies and author CSS have already been copied into the supplied
/// resource registry. Their URLs are passed to the view for execution.
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

    /// The registered background-thread module URL, when the section was present.
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

/// Failure to classify or decode a page container.
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
    /// Two of a container's names reach one URL with different texts, or a
    /// stylesheet's URL is also a script's.
    #[error("container `{input}` names `{url}` twice, with different contents")]
    ConflictingSection { input: String, url: String },
    /// A container URL that cannot be a base, so no section hangs under it.
    #[error("container URL `{0}` cannot be a base for its sections")]
    OpaqueContainerUrl(String),
}

/// Explicit name for [`SourceError`] at APIs that carry several error types.
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
    /// The main-thread script, as the module registered for it.
    main_thread: (Url, String),
    style: Option<(Url, &'source str)>,
    /// The background-thread script, as the BTS entry registered for it.
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
    /// Decodes a page the caller has already loaded: a container with a
    /// `root`, or Lynx XML.
    ///
    /// # Errors
    ///
    /// [`SourceError::MissingRoot`] for a web container with no `root`, and
    /// [`SourceError::MissingNativeEntry`] for a native one; every error
    /// [`ContainerSources::from_bytes`] or the XML parser has.
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

    /// Decode a source-based native external bundle and select its named main module.
    /// The ordinary byte-sniffing path requires `root`; external libraries normally
    /// need an explicit name such as `library__main-thread` instead.
    ///
    /// # Errors
    ///
    /// As [`ContainerSources::from_native_bundle`].
    pub fn from_native_bundle(input: &Url, bytes: &[u8], entry: &str) -> Result<Self, SourceError> {
        Self::from_container(
            input,
            ContainerSources::from_native_bundle(input, bytes, entry)?,
        )
    }

    /// A container page, which a container is only with an entry.
    fn from_container(input: &Url, container: ContainerSources) -> Result<Self, SourceError> {
        if container.entry().is_none() {
            return Err(SourceError::MissingRoot(diagnostic_url(input)));
        }
        Ok(Self {
            input_url: input.clone(),
            page: Page::Container(container),
        })
    }

    /// A page from an already-decoded template, as either compiler's
    /// container: the tests' way to give one body every shape.
    #[cfg(test)]
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

    /// The URL that identifies the decoded input.
    #[must_use]
    pub const fn input_url(&self) -> &Url {
        &self.input_url
    }

    /// Page policy decoded from this input's configuration.
    #[must_use]
    pub const fn config(&self) -> PageConfig {
        match &self.page {
            Page::Container(container) => container.config(),
            Page::Xml(_) => raw_lynx_xml_config(),
        }
    }

    /// The container a container page registers, for the tests that hold it
    /// up against what a fetch of the same bytes registers.
    #[cfg(test)]
    pub(crate) const fn container(&self) -> Option<&ContainerSources> {
        match &self.page {
            Page::Container(container) => Some(container),
            Page::Xml(_) => None,
        }
    }

    /// Registers the decoded scripts and stylesheets with an embedder-owned
    /// resource system: a container's through
    /// [`ContainerSources::register_with`], an XML page's two scripts and its
    /// CSS as they are.
    ///
    /// Every registration URL was derived from an already-parsed [`Url`], so
    /// registration cannot fail URL validation. `Resources::register`
    /// replaces an earlier registration of the same URL, so two pages
    /// registering one input URL leave the later one's bodies — and whatever
    /// URL only the earlier one had.
    pub fn register_with(&self, resources: &Resources) {
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

    /// The sources a view for this input is built from: its entry MTS module,
    /// its BTS entry if any, the author CSS this input carried, if any, and —
    /// for a container — the container URL as the card, reporting `screen` as
    /// its `SystemInfo`.
    ///
    /// The view's base URL is the input URL, the base an embedder gives its
    /// fetcher for this input, so the view and the fetcher resolve a
    /// container's URL alike. The entries are already absolute URLs, which
    /// resolve to themselves. A container's named stylesheets are not listed:
    /// a card adopts them through `__LoadStyleSheet`.
    ///
    /// The screen is the host's to name, since no input carries one: the
    /// monitor or browser screen it measured, or
    /// [`ScreenMetrics::for_viewport`] of its capture size where it has none.
    #[must_use]
    pub fn view_sources(&self, screen: ScreenMetrics) -> ViewSources {
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
                    ..ViewSources::new(input, entry.to_string(), screen)
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
                ..ViewSources::new(input, xml.entry.0.to_string(), screen)
            },
        }
    }
}

/// Which compiler built one container, and so what shape its bodies arrive in.
///
/// A `.web.bundle`'s are `CommonJS` files. A source-based `.lynx.bundle`'s are
/// *expression statements* — the Lynx compiler's own `(function(){…})()`, or a
/// `RuntimeWrapperWebpackPlugin` banner — whose value native's host keeps as
/// the completion value of the script it evaluated them as, and hands
/// lynx-core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleTarget {
    Lynx,
    Web,
}

/// One bundle body as the ES module a realm `require`s it as.
///
/// Both shapes answer through the module's **default export**, which is the
/// one thing `bobcat:lynx-modules` reads, and both keep the body starting on
/// the line it started on: every prefix is one physical line, and only the
/// `CommonJS` suffix adds one, after the body. So an error reports the line
/// it had in the container, and on any line after the first the column too.
/// On the body's first line the column is offset by the prefix's length —
/// [`bobcat_core::BTS_CHUNK_PREAMBLE`] and then `export default ` or the
/// `CommonJS` declarations — which for a minified body written on one line
/// is every column it has.
///
/// - [`BundleTarget::Lynx`]: `export default <body>`. The body is one expression, so what native
///   would have kept as its script's completion value — normally the `{init}` object the compiler's
///   IIFE returns — becomes the module's default export instead.
/// - [`BundleTarget::Web`]: a `module` object and an `exports` alias of its own, the body, then
///   `export default module.exports`.
///
/// A `.json` body is neither: it is a value, and its own URL is what tells
/// the loader to parse it, so `crate::container` registers it verbatim.
///
/// A raw body's leading `"use strict"` stops being a directive prologue under
/// either, because something precedes it. Nothing is lost: a module is strict
/// already.
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

/// A card's MTS body as the module registered for it:
/// [`bobcat_core::MTS_CHUNK_PREAMBLE`], then the body.
///
/// This is where a card's main-thread script gets the bindings it expects to
/// find in scope — the runtime names and the Element PAPI — because the engine
/// adds nothing to an entry: it completes the entry with what the fetcher
/// answered. Every MTS body this crate registers as a module is written by
/// this, or, for a container's `main-thread` section, by the same preamble in
/// front of an `export default`.
///
/// The preamble is one physical line and the body starts on it, as in
/// [`bts_module_source`], so every line of the body keeps the number it had
/// in the container, and an error on any line after the first reports the
/// column it had there too. An error on the body's first line reports a
/// column offset by the preamble's length, which for a minified body written
/// on one line is every column it has. A body's leading `"use strict"` stops
/// being a directive prologue, because something precedes it. Nothing is
/// lost: a module is strict already.
#[must_use]
pub fn mts_entry_source(body: &str) -> String {
    let mut source = String::with_capacity(bobcat_core::MTS_CHUNK_PREAMBLE.len() + body.len());
    source.push_str(bobcat_core::MTS_CHUNK_PREAMBLE);
    source.push_str(body);
    source
}

/// An XML page's background-thread script as the BTS entry registered for it:
/// a [`BundleTarget::Web`] body, through [`bts_module_source`].
///
/// web-core's `xmlToTasmJSON` puts the script, verbatim, at its bundle's
/// `/app-service.js`, and its `createChunkLoading` runs every BTS chunk inside
/// its chunk wrapper, so the script sees the names that wrapper passes, a
/// `module` object among them. This is that wrapper here. The script is the
/// view's BTS entry rather than a manifest path, so nothing reads the
/// `module.exports` it leaves.
fn xml_background_source(body: &str) -> String {
    bts_module_source(BundleTarget::Web, body)
}

/// The BTS boot script: the container's own URL, then the card.
///
/// Nothing of the container is in it — no body's name, no body's URL and no
/// body's text. The template URL is the container's own URL, which every
/// manifest path hangs under, and `requireModule` is what loads and
/// evaluates `/app-service.js` under it, synchronously, from inside this
/// module's own evaluation. No entry is named: this bundle registers under the
/// default entry either way.
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

/// Parses and registers one already-decoded browser Lynx XML response.
///
/// Section identities are fragments of `input`, which must be the final
/// response URL so relative imports and CSS URLs retain the browser-observed
/// redirect base. `source` is already Unicode: replacement characters emitted
/// by the browser's UTF-8 decoder are ordinary contents here. Both script bodies
/// and author CSS are copied into `resources` for the view to load: each script
/// as the module a card body becomes here — the main-thread script through
/// [`mts_entry_source`], the background-thread script as a
/// [`BundleTarget::Web`] body — and the CSS as it is, exactly as
/// [`PageSource::register_with`] registers an XML page's.
pub fn register_lynx_xml_response(
    input: &Url,
    source: &str,
    resources: &Resources,
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
    // Both scripts are card bodies, wrapped here for both URL policies.
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

/// Recognizes native magics before the input can fall through to web decoding.
/// The native decoder validates the leading total size and section structure.
pub(crate) fn looks_like_native_bundle(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..8) else {
        return false;
    };
    let magic = u32::from_le_bytes(header[4..].try_into().expect("four-byte native magic"));
    matches!(magic, 0x0024_1922 | 0xdd73_7199)
}

/// Whether these bytes open with the `SDRA WROF` pair [`crate::web::decode`]
/// checks — the two little-endian magics a `.web.bundle` starts with.
///
/// Sniffing, not validation: a file that starts this way is a web container
/// as far as anything here can tell, and the decoder is what says whether it
/// really is one.
pub(crate) fn looks_like_web_bundle(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..8) else {
        return false;
    };
    let magic0 = u32::from_le_bytes(header[..4].try_into().expect("four-byte web magic"));
    let magic1 = u32::from_le_bytes(header[4..].try_into().expect("four-byte web magic"));
    magic0 == crate::web::MAGIC_0 && magic1 == crate::web::MAGIC_1
}

/// Mirrors web-core's raw-input classification: any run of ASCII whitespace
/// and UTF-8 BOMs is ignored for sniffing, while the XML parser itself remains
/// responsible for enforcing its stricter single-leading-BOM grammar.
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use bobcat_resources::ResourcesConfig;

    use super::*;

    fn input_url() -> Url {
        Url::parse("file:///tmp/card.lynx.xml").expect("test URL")
    }

    /// The input a container page is decoded from in these tests: a
    /// container is named by what it is.
    fn bundle_url() -> Url {
        Url::parse("file:///tmp/card.web.bundle").expect("test URL")
    }

    /// The text a page registers at `url`.
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

    /// The text a page registers for its view's entry.
    fn entry_script(page: &PageSource) -> &str {
        script_at(page, &page.view_sources(SCREEN).entry)
    }

    /// The text a page registers for its view's BTS entry, if it has one.
    fn background_script(page: &PageSource) -> Option<&str> {
        page.view_sources(SCREEN)
            .background_entry
            .map(|url| script_at(page, &url))
    }

    /// An XML page's parts.
    fn xml(page: &PageSource) -> &XmlPage {
        match &page.page {
            Page::Xml(xml) => xml,
            Page::Container(_) => panic!("a container page"),
        }
    }

    /// The screen these tests' views report: their own 32×24 viewport, as a
    /// host with no screen to measure names it.
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
        let sources = page.view_sources(SCREEN);
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
        assert!(sources.background_entry.is_none());
        assert!(sources.style_sheets.is_empty());

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

        let sources = page.view_sources(SCREEN);
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

    /// A `CommonJS` body's module: a `module` object of its own on the body's
    /// own first line, and the export of what it left there afterwards.
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

    /// A `.lynx.bundle` body's module: the expression exported as it stands,
    /// its own trailing `;` and source map comment tolerated by `export
    /// default <expr>;`.
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

    /// The preamble is one physical line under either shape, so a body's own
    /// line numbers survive registration.
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

    /// A JSON body is a value rather than a file, and is registered as the
    /// text it is whatever the container was built by: the loader reads a
    /// `.json` response as JSON, so there is nothing for a module wrapper to
    /// be compiled as.
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

    /// A native container addresses its main-thread sources twice: as Lepus
    /// chunks, and as custom sections of the same name. Either way it is an
    /// MTS section, not a BTS body, and the boot script names none of them.
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

    /// A body of three lines, to read line numbers off.
    const THREE_LINES: &str = "first();\nsecond();\nthird();";

    /// `source` is `MTS_CHUNK_PREAMBLE` and then [`THREE_LINES`], with the
    /// body's first line on the preamble's own: line 1 of the module.
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

    /// The engine adds nothing to an entry, so a container's root Lepus
    /// script is registered with the imports a card's MTS body is given in
    /// front of it, on its own first line — whichever compiler built the
    /// container. A `.lynx.bundle` reaches this through
    /// `from_native_bundle`, which names its entry `root` first.
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

    /// Both XML paths register the main-thread script the way a container's
    /// root is registered, and the background-thread script as the `Web`
    /// body web-core's chunk wrapper would have run it as.
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

        // `register_lynx_xml_response` registers what this maps, unchanged.
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
        // The root is the container's own text after `MTS_CHUNK_PREAMBLE`.
        // Nothing else is prefixed to it: not an import of a chunk, not a call
        // registering one.
        assert_eq!(entry_script(&page), mts_entry_source("export {};"));

        let resources = resources();
        page.register_with(&resources);
        assert!(
            resources.unregister("https://example.test/card.web.bundle/A%20%26%2F%E4%B8%AD.js")
        );
    }

    /// An offscreen painter at the size the boot tests here use.
    ///
    /// Nothing draws through it: boot's first `__FlushElementTree` waits for a
    /// painter to bind the view, and binding it is all this is for.
    #[cfg(not(target_arch = "wasm32"))]
    async fn binding_painter() -> bobcat_core::Painter {
        bobcat_core::Painter::new(bobcat_core::DrawTarget::Offscreen, 32.0, 24.0, 1.0)
            .await
            .unwrap()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn registered_compiler_sources_boot_bts_for_web_and_native_wrappers() {
        use std::time::{Duration, Instant};

        use bobcat_core::{EngineEvent, LynxGroup, NoWakeup, StyleThreads};

        // What the card does once something calls its `init`: define a module
        // through the factory ABI, require it, and reach the rest of its
        // container on the way. Shared by both containers, because the factory
        // ABI is the same either way — what differs is only how the body hands
        // its `{init}` over.
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

        // Each of the three `.lynx.bundle` body shapes below, answered by its
        // own name: they are imported by the boot script whatever happens, so
        // a shape that did not compile as a module would fail the boot — this
        // also reads the `{init}` each of them evaluated to.
        const SHAPES: &str = r"
            for (const shape of ['compact', 'minified', 'pretty'])
                if (lynx.loadScript(shape, {}) !== shape) throw Error('banner ' + shape);
        ";

        // The two containers. A `.lynx.bundle`'s bodies are expression
        // statements — the three shapes the Lynx compiler writes are all
        // exercised below — and a `.web.bundle`'s are `CommonJS` files. Both
        // hand an `{init}` over, which is what a compiled card is: the body's
        // own evaluation defines nothing, and `requireModule` is what starts
        // the card.
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
                    // `LynxEncodePlugin`'s shape, down to the banner's
                    // trailing `;` and source map comment.
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
            // A section is a body of its container like any other: an
            // expression under Lynx, a `CommonJS` file under Web. Its value is
            // no `{init}`, so it is what `loadScript` answers as it stands —
            // web-core's `createBundleInitReturnObj` result rather than
            // native's Script completion value.
            let mut sections = serde_json::json!({
                "answer": {"content": match target {
                    BundleTarget::Web => "module.exports = 21 * 2",
                    BundleTarget::Lynx => "21 * 2",
                }},
            });
            if matches!(target, BundleTarget::Lynx) {
                // The three shapes a `.lynx.bundle` carries, each of which has
                // to compile as a module: the compact `LynxEncodePlugin` IIFE
                // with no trailing `;`, a minified `RuntimeWrapperWebpackPlugin`
                // banner ending `})();` and a source map comment, and the same
                // banner unminified, with whitespace around its tail.
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
                    page.view_sources(SCREEN),
                )
                .unwrap();
            // Boot's first flush waits for a painter to bind the view.
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

    /// One path, proven: a page's container is exactly what a fetch of the
    /// same bytes from the same URL registers — every script at the same URL
    /// with the same text, the same stylesheets, the same entry and boot.
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

    /// A native page names its entry explicitly, which a fetch cannot: the
    /// page is the fetch plus exactly that entry at `root.js` and the boot
    /// script that starts its card — nothing else differs.
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

    /// A container page boots with the container as its card: `__Card__` is
    /// the input URL rather than the entry's, and a named chunk and a named
    /// stylesheet are found under it by the URLs MTS builds from it.
    #[cfg(not(target_arch = "wasm32"))]
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
        let sources = page.view_sources(SCREEN);
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

    /// `enableCSSInheritance` is read the way web-core reads it
    /// (`config['enableCSSInheritance'] === 'true'`, lynx-stack #3907): the
    /// string `"true"` turns it on, and `"false"`, an absent key and any other
    /// value leave it off. The web encoder writes every page config value
    /// through `String(value)`, so `"true"` is the only spelling a
    /// `.web.bundle` has for an enabled switch.
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
                page.view_sources(SCREEN).config.enable_css_inheritance,
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
        // Present and empty: a `Web` body with nothing in it.
        assert_eq!(
            background_script(&page),
            Some(bts_module_source(BundleTarget::Web, "").as_str())
        );
        assert!(page.view_sources(SCREEN).card_url.is_none());
        let sources = page.view_sources(SCREEN);
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
        assert!(registered.compatibility_warnings().is_empty());
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
        assert!(page.view_sources(SCREEN).background_entry.is_none());
        assert_eq!(
            page.view_sources(SCREEN).entry,
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
}
