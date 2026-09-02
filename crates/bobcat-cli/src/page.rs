//! Loading a `.web.bundle` or raw Lynx XML into the values the engine consumes.
//!
//! This is embedder IO: read and sniff the bytes, decode the selected source
//! container, retain its scripts and author CSS behind the resource-fetching
//! contract, and hand the root URL plus page config to Bobcat. The pipeline
//! itself — tree, commits, style, layout, paint, scheduling — remains the
//! engine's.

use std::sync::Arc;

use bobcat_core::resource::{
    CacheStatus, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceLocality, ResourceMetadata,
    ResourceRequest, ResourceResponse, ResourceSource, ResourceTiming, RetryAdvice,
    StyleSheetPayload, StyleSheetResponse,
};
use bobcat_core::{ImageReports, PageConfig, PreparsedStyleSheet, ViewSources};
use http::HeaderMap;
use url::Url;

use crate::CliError;

#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) input: String,
    script_url: Url,
    /// The conventional background chunk URL for raw XML that contains a
    /// background section. Its presence is retained even when the source is
    /// empty; the current runtime does not execute it.
    background_script_url: Option<Url>,
    /// The non-global CSS fragment ids a decoded bundle carries, if any.
    scoped_css_ids: Vec<i32>,
    pub(crate) resource_fetcher: ProgramResourceFetcher,
    pub(crate) config: PageConfig,
}

/// The CLI's resource provider for sources extracted from one input.
///
/// Container loading and decoding/parsing deliberately stay in the embedder.
/// The engine sees the root script through the same URL-based resource
/// boundary that a networked or packaged embedder would implement.
#[derive(Clone, Debug)]
pub(crate) struct ProgramResourceFetcher {
    script_url: Url,
    source: Arc<str>,
    background_script: Option<(Url, Arc<str>)>,
    style_sheet_url: Option<Url>,
    style_sheet: Option<ProgramStyleSheet>,
}

/// Author CSS in the most direct form supplied by the input container.
#[derive(Clone, Debug)]
enum ProgramStyleSheet {
    /// Verbatim UTF-8 CSS from a raw Lynx XML `<style>` section.
    Text(Arc<str>),
    /// A web bundle's rkyv `StyleInfo`, lowered without reserializing it.
    Preparsed(Arc<PreparsedStyleSheet>),
}

impl ProgramResourceFetcher {
    fn new(script_url: Url, source: String) -> Self {
        Self {
            script_url,
            source: Arc::from(source),
            background_script: None,
            style_sheet_url: None,
            style_sheet: None,
        }
    }

    fn with_background_script(mut self, url: Url, source: String) -> Self {
        self.background_script = Some((url, Arc::from(source)));
        self
    }

    fn with_preparsed_style_sheet(mut self, url: Url, sheet: PreparsedStyleSheet) -> Self {
        self.style_sheet_url = Some(url);
        self.style_sheet = Some(ProgramStyleSheet::Preparsed(Arc::new(sheet)));
        self
    }

    fn with_text_style_sheet(mut self, url: Url, source: String) -> Self {
        self.style_sheet_url = Some(url);
        self.style_sheet = Some(ProgramStyleSheet::Text(Arc::from(source)));
        self
    }

    /// The URL the input's author CSS is registered under, if it carried any.
    ///
    /// The registration is the single source of truth: an input whose sheet
    /// was never registered has no URL to load, and one that was cannot be
    /// missed.
    fn style_sheet_url(&self) -> Option<&Url> {
        self.style_sheet_url.as_ref()
    }

    fn error(
        request_id: Option<RequestId>,
        kind: ResourceErrorKind,
        phase: ResourceErrorPhase,
        locator: Option<Arc<str>>,
        message: &'static str,
    ) -> ResourceError {
        ResourceError {
            request_id,
            kind,
            phase,
            locator,
            status: None,
            message: Arc::from(message),
            retry: RetryAdvice::Never,
        }
    }
}

impl ResourceFetcher for ProgramResourceFetcher {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        match capability {
            ResourceCapability::BufferedResource => true,
            ResourceCapability::PreparsedStyleSheet => matches!(
                self.style_sheet.as_ref(),
                Some(ProgramStyleSheet::Preparsed(_))
            ),
            _ => false,
        }
    }

    async fn resolve_locator(
        &self,
        request: ResolveRequest,
    ) -> Result<ResolvedLocator, ResourceError> {
        let request_id = request.context.id;
        let locator = request.resource.specifier.clone();

        let resolved_url = Url::parse(&locator).or_else(|_| {
            request
                .resource
                .base_url
                .as_ref()
                .ok_or(url::ParseError::RelativeUrlWithoutBase)
                .and_then(|base_url| base_url.join(&locator))
        });
        let Ok(url) = resolved_url else {
            return Err(Self::error(
                Some(request_id),
                ResourceErrorKind::InvalidUrl,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "resource locator is not a valid URL",
            ));
        };
        let is_background_script = self
            .background_script
            .as_ref()
            .is_some_and(|(script_url, _)| &url == script_url);
        if url != self.script_url
            && !is_background_script
            && Some(&url) != self.style_sheet_url.as_ref()
        {
            return Err(Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "resource is not present in the decoded input",
            ));
        }

        let resource = request.resource;
        let cache_key = Some(Arc::from(url.as_str()));
        Ok(ResolvedLocator {
            resource,
            url,
            rewrite_chain: Vec::new(),
            locality: ResourceLocality::Local,
            cache_key,
        })
    }

    async fn fetch_resource(
        &self,
        request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError> {
        let request_id = request.context.id;
        let locator: Arc<str> = Arc::from(request.resource.url.as_str());
        let source = if request.resource.url == self.script_url {
            self.source.clone()
        } else if let Some((_, source)) = self
            .background_script
            .as_ref()
            .filter(|(url, _)| &request.resource.url == url)
        {
            source.clone()
        } else {
            return Err(Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Open,
                Some(locator),
                "resource is not present in the decoded input",
            ));
        };
        let content_length = source.len() as u64;

        let resource = request.resource;
        Ok(ResourceResponse {
            metadata: ResourceMetadata {
                request_id,
                resource,
                headers: HeaderMap::default(),
                content_length: Some(content_length),
                media_type: Some(Arc::from("text/javascript; charset=utf-8")),
                source: ResourceSource::MemoryCache,
                cache_status: CacheStatus::default(),
                timing: ResourceTiming::default(),
            },
            bytes: source.as_bytes().to_vec().into(),
        })
    }

    async fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> Result<StyleSheetResponse, ResourceError> {
        let request_id = request.context.id;
        let locator: Arc<str> = Arc::from(request.resource.url.as_str());
        let Some(sheet) = self
            .style_sheet
            .clone()
            .filter(|_| Some(&request.resource.url) == self.style_sheet_url.as_ref())
        else {
            return Err(Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Open,
                Some(locator),
                "the decoded input carries no stylesheet at this URL",
            ));
        };
        let (content_length, payload) = match sheet {
            ProgramStyleSheet::Text(source) => {
                let content_length = source.len() as u64;
                (
                    Some(content_length),
                    StyleSheetPayload::Text(source.as_bytes().to_vec().into()),
                )
            }
            ProgramStyleSheet::Preparsed(sheet) => (None, StyleSheetPayload::Preparsed(sheet)),
        };
        let resource = request.resource;
        Ok(StyleSheetResponse {
            metadata: ResourceMetadata {
                request_id,
                resource,
                headers: HeaderMap::default(),
                content_length,
                media_type: Some(Arc::from("text/css; charset=utf-8")),
                source: ResourceSource::MemoryCache,
                cache_status: CacheStatus::default(),
                timing: ResourceTiming::default(),
            },
            payload,
        })
    }
}

/// The CLI serves no images: a page it renders draws whatever its own
/// stylesheet describes, and nothing fetches a bitmap. Every image draw
/// therefore resolves to nothing, which is what an unloaded image looks like
/// anyway.
impl bobcat_core::FrameImages for ProgramResourceFetcher {
    fn read(&self, _source: &str) -> Option<bobcat_core::vello::peniko::ImageData> {
        None
    }
}

impl Program {
    pub(crate) fn load(input: &Url) -> Result<Self, CliError> {
        let path = input
            .to_file_path()
            .map_err(|()| CliError::InputUrl(input.to_string()))?;
        let bytes = std::fs::read(&path).map_err(|source| CliError::ReadInput {
            path: path.clone(),
            source,
        })?;
        Self::from_bytes(input, &bytes)
    }

    fn from_bytes(input: &Url, bytes: &[u8]) -> Result<Self, CliError> {
        if looks_like_lynx_xml(bytes) {
            Self::from_lynx_xml(input, bytes)
        } else {
            Self::from_web_bundle(input, bytes)
        }
    }

    fn from_web_bundle(input: &Url, bytes: &[u8]) -> Result<Self, CliError> {
        let mut template =
            lynx_template_decoder::decode(bytes).map_err(|source| CliError::Decode {
                input: input.to_string(),
                source,
            })?;
        let source = template
            .lepus_code
            .remove("root")
            .ok_or_else(|| CliError::MissingRoot(input.to_string()))?;
        let script_url = Url::parse("bobcat-memory://bundle/lepus-root.js")
            .expect("the built-in root-script URL must be valid");
        let mut fetcher = ProgramResourceFetcher::new(script_url.clone(), source);
        let style_sheet = template
            .style_info
            .as_ref()
            .map(crate::style_info::to_preparsed_style_sheet)
            .filter(|sheet| !sheet.is_empty());
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
        if let Some(sheet) = style_sheet {
            let url = Url::parse("bobcat-memory://bundle/style-info.css")
                .expect("the built-in stylesheet URL must be valid");
            fetcher = fetcher.with_preparsed_style_sheet(url, sheet);
        }
        let config = PageConfig {
            default_display_linear: template.config_flag("defaultDisplayLinear"),
            default_overflow_visible: template.config_flag("defaultOverflowVisible"),
            enable_css_selector: template.config_flag("enableCSSSelector"),
        };
        Ok(Self {
            input: input.to_string(),
            script_url,
            background_script_url: None,
            scoped_css_ids,
            resource_fetcher: fetcher,
            config,
        })
    }

    fn from_lynx_xml(input: &Url, bytes: &[u8]) -> Result<Self, CliError> {
        let source =
            std::str::from_utf8(bytes).map_err(|source| CliError::InvalidLynxXmlEncoding {
                input: input.to_string(),
                source,
            })?;
        let xml = lynx_xml::parse(source).map_err(|source| CliError::ParseLynxXml {
            input: input.to_string(),
            source,
        })?;
        let script_url = Url::parse("bobcat-memory://lynx-xml/main-thread.js")
            .expect("the built-in XML main-script URL must be valid");
        let mut fetcher =
            ProgramResourceFetcher::new(script_url.clone(), xml.main_thread_script.to_owned());

        let background_script_url = if let Some(source) = xml.background_thread_script {
            let url = Url::parse("bobcat-memory://lynx-xml/app-service.js")
                .expect("the built-in XML background-script URL must be valid");
            fetcher = fetcher.with_background_script(url.clone(), source.to_owned());
            Some(url)
        } else {
            None
        };
        if let Some(source) = xml.style {
            let url = Url::parse("bobcat-memory://lynx-xml/style.css")
                .expect("the built-in XML stylesheet URL must be valid");
            fetcher = fetcher.with_text_style_sheet(url, source.to_owned());
        }

        Ok(Self {
            input: input.to_string(),
            script_url,
            background_script_url,
            scoped_css_ids: Vec::new(),
            resource_fetcher: fetcher,
            config: PageConfig {
                default_display_linear: false,
                default_overflow_visible: false,
                enable_css_selector: true,
            },
        })
    }

    /// The sources a view for this input is built from: the author CSS this
    /// input carried, if any, and its entry MTS module.
    pub(crate) fn sources(&self) -> ViewSources {
        ViewSources {
            config: self.config,
            style_sheets: self
                .resource_fetcher
                .style_sheet_url()
                .map(Url::to_string)
                .into_iter()
                .collect(),
            ..ViewSources::new(self.script_url.to_string())
        }
    }

    /// Builds the resource system this input is served by, for the painter
    /// to own. The CLI loads no image asynchronously, so it has nothing to
    /// report and drops the sink.
    pub(crate) fn resources(&self) -> impl FnOnce(ImageReports) -> ProgramResourceFetcher {
        let fetcher = self.resource_fetcher.clone();
        move |_sink| fetcher
    }

    /// Reports input features the current runtime retains only approximately
    /// or cannot execute yet.
    ///
    /// A bundle compiled with `enableRemoveCSSScope = false` keeps one
    /// fragment per component and expects each fragment's rules to match only
    /// inside that component. Per-component scoping is not implemented, so
    /// those rules mount globally and two components that style the same class
    /// name will collide. Rendering them is still better than rendering
    /// nothing, but it is not silent.
    pub(crate) fn warn_about_compatibility_limits(&self) {
        if !self.scoped_css_ids.is_empty() {
            let ids: Vec<String> = self
                .scoped_css_ids
                .iter()
                .map(ToString::to_string)
                .collect();
            eprintln!(
                "bobcat: warning: {} carries component-scoped CSS fragments (css ids {}); per-component \
                 scoping is not implemented, so their rules apply globally",
                self.input,
                ids.join(", ")
            );
        }
        if let Some(url) = self.background_script_url.as_ref() {
            eprintln!(
                "bobcat: warning: {} carries a Lynx XML background-thread script at {}; \
                 background-thread JavaScript is retained but not executed",
                self.input,
                url.path()
            );
        }
    }
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
    use super::*;

    fn input_url() -> Url {
        Url::parse("file:///tmp/card.lynx.xml").expect("test URL")
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
    fn xml_preserves_present_empty_style_and_background_sections() {
        let program = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><style></style><script thread=\"main\">main</script><script thread=\"background\"></script></lynx>",
        )
        .expect("valid XML program");

        assert!(!program.config.default_display_linear);
        assert!(!program.config.default_overflow_visible);
        assert!(program.config.enable_css_selector);
        assert_eq!(
            program.resource_fetcher.style_sheet_url().map(Url::path),
            Some("/style.css")
        );
        assert_eq!(
            program.background_script_url.as_ref().map(Url::path),
            Some("/app-service.js")
        );
        assert!(matches!(
            program.resource_fetcher.style_sheet.as_ref(),
            Some(ProgramStyleSheet::Text(source)) if source.is_empty()
        ));
        assert!(matches!(
            program.resource_fetcher.background_script.as_ref(),
            Some((_, source)) if source.is_empty()
        ));
    }

    #[test]
    fn xml_without_optional_sections_keeps_them_absent() {
        let program = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">main</script></lynx>",
        )
        .expect("valid XML program");

        assert!(program.resource_fetcher.style_sheet_url().is_none());
        assert!(program.background_script_url.is_none());
    }

    #[test]
    fn sniffed_xml_rejects_invalid_utf8_before_parsing() {
        let error = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">\xff</script></lynx>",
        )
        .expect_err("invalid UTF-8 must fail");

        assert!(matches!(error, CliError::InvalidLynxXmlEncoding { .. }));
    }

    #[test]
    fn malformed_sniffed_xml_reports_an_xml_parse_error() {
        let error = Program::from_bytes(
            &input_url(),
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">main</script>",
        )
        .expect_err("malformed XML must fail");

        assert!(matches!(error, CliError::ParseLynxXml { .. }));
    }
}
