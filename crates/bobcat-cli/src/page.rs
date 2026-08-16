//! Loading a `.web.bundle` into the plain values the engine consumes.
//!
//! This is embedder IO: read the bytes, decode the template, retain its root
//! script behind the resource-fetching contract, and hand its URL plus page
//! config to Bobcat. The pipeline itself — tree, commits, style, layout,
//! paint, scheduling — remains the engine's.

use std::sync::Arc;

use bobcat_core::resource::{
    BufferedResourceRequest, CacheStatus, HttpRequest, HttpResponse, PrefetchReceipt,
    PrefetchRequest, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourceLocality,
    ResourceMetadata, ResourcePath, ResourceRequest, ResourceResponse, ResourceSource,
    ResourceStream, ResourceTiming, RetryAdvice, StyleSheetPayload, StyleSheetResponse,
};
use bobcat_core::{PageConfig, PreparsedStyleSheet};
use http::HeaderMap;
use url::Url;

use crate::CliError;

#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) input: String,
    pub(crate) script_url: Url,
    /// The non-global CSS fragment ids this bundle carries, if any.
    scoped_css_ids: Vec<i32>,
    pub(crate) resource_fetcher: Arc<ProgramResourceFetcher>,
    pub(crate) config: PageConfig,
}

/// The CLI's resource provider for sources extracted from one decoded bundle.
///
/// Bundle loading and decoding deliberately stay in the embedder. The engine
/// sees the root script through the same URL-based resource boundary that a
/// networked or packaged embedder would implement.
#[derive(Clone, Debug)]
pub(crate) struct ProgramResourceFetcher {
    script_url: Url,
    source: Arc<str>,
    style_sheet_url: Option<Url>,
    /// The bundle's author CSS, already lowered out of its rkyv wire form.
    /// Answering the stylesheet request with this rather than CSS text is the
    /// point of [`ResourceCapability::PreparsedStyleSheet`].
    style_sheet: Option<Arc<PreparsedStyleSheet>>,
}

impl ProgramResourceFetcher {
    fn new(script_url: Url, source: String) -> Self {
        Self {
            script_url,
            source: Arc::from(source),
            style_sheet_url: None,
            style_sheet: None,
        }
    }

    fn with_style_sheet(mut self, url: Url, sheet: PreparsedStyleSheet) -> Self {
        self.style_sheet_url = Some(url);
        self.style_sheet = Some(Arc::new(sheet));
        self
    }

    /// The URL the bundle's author CSS is registered under, if it carried any.
    ///
    /// The registration is the single source of truth: a bundle whose sheet was
    /// never registered has no URL to load, and one that was cannot be missed.
    pub(crate) fn style_sheet_url(&self) -> Option<&Url> {
        self.style_sheet_url.as_ref()
    }

    fn error<T>(
        request_id: Option<RequestId>,
        kind: ResourceErrorKind,
        phase: ResourceErrorPhase,
        locator: Option<Arc<str>>,
        message: &'static str,
    ) -> ResourceFuture<'static, T> {
        Box::pin(async move {
            Err(ResourceError {
                request_id,
                kind,
                phase,
                locator,
                status: None,
                message: Arc::from(message),
                retry: RetryAdvice::Never,
            })
        })
    }

    fn unsupported<T>(
        request_id: Option<RequestId>,
        phase: ResourceErrorPhase,
    ) -> ResourceFuture<'static, T> {
        Self::error(
            request_id,
            ResourceErrorKind::UnsupportedOperation,
            phase,
            None,
            "the CLI in-memory fetcher does not support this operation",
        )
    }
}

impl ResourceFetcher for ProgramResourceFetcher {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        match capability {
            ResourceCapability::BufferedResource => true,
            ResourceCapability::PreparsedStyleSheet => self.style_sheet.is_some(),
            _ => false,
        }
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        let request_id = request.context.id;
        let locator = request.resource.locator.specifier.clone();
        if request.context.cancellation.is_cancelled() {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::Cancelled,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "resource resolution was cancelled",
            );
        }

        let resolved_url = Url::parse(&locator).or_else(|_| {
            request
                .resource
                .locator
                .base_url
                .as_ref()
                .ok_or(url::ParseError::RelativeUrlWithoutBase)
                .and_then(|base_url| base_url.join(&locator))
        });
        let Ok(url) = resolved_url else {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::InvalidUrl,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "resource locator is not a valid URL",
            );
        };
        if url != self.script_url && Some(&url) != self.style_sheet_url.as_ref() {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "resource is not present in the decoded bundle",
            );
        }

        let resource = request.resource;
        let cache_key = Some(Arc::from(url.as_str()));
        Box::pin(async move {
            Ok(ResolvedLocator {
                resource,
                url,
                rewrite_chain: Vec::new(),
                locality: ResourceLocality::Local,
                cache_key,
            })
        })
    }

    fn fetch_resource(
        &self,
        request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, ResourceResponse> {
        let request_id = request.request.context.id;
        let locator: Arc<str> = Arc::from(request.request.resource.url.as_str());
        if request.request.context.cancellation.is_cancelled() {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::Cancelled,
                ResourceErrorPhase::Open,
                Some(locator),
                "resource fetch was cancelled",
            );
        }
        if request.request.resource.url != self.script_url {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Open,
                Some(locator),
                "resource is not present in the decoded bundle",
            );
        }
        let content_length = self.source.len() as u64;
        if content_length > request.max_bytes {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::ResponseTooLarge,
                ResourceErrorPhase::ReadBody,
                Some(locator),
                "root script exceeds the caller's buffered-resource limit",
            );
        }

        let source = self.source.clone();
        let resource = request.request.resource;
        Box::pin(async move {
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
        })
    }

    fn fetch_style_sheet(
        &self,
        request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, StyleSheetResponse> {
        let request_id = request.request.context.id;
        let locator: Arc<str> = Arc::from(request.request.resource.url.as_str());
        let Some(sheet) = self
            .style_sheet
            .clone()
            .filter(|_| Some(&request.request.resource.url) == self.style_sheet_url.as_ref())
        else {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Open,
                Some(locator),
                "the decoded bundle carries no stylesheet at this URL",
            );
        };
        let resource = request.request.resource;
        Box::pin(async move {
            Ok(StyleSheetResponse {
                metadata: ResourceMetadata {
                    request_id,
                    resource,
                    headers: HeaderMap::default(),
                    content_length: None,
                    media_type: Some(Arc::from("text/css; charset=utf-8")),
                    source: ResourceSource::MemoryCache,
                    cache_status: CacheStatus::default(),
                    timing: ResourceTiming::default(),
                },
                payload: StyleSheetPayload::Preparsed(sheet),
            })
        })
    }

    fn open_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceStream> {
        Self::unsupported(Some(request.context.id), ResourceErrorPhase::Open)
    }

    fn fetch_resource_path(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourcePath> {
        Self::unsupported(
            Some(request.context.id),
            ResourceErrorPhase::MaterializePath,
        )
    }

    fn fetch_http(&self, request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        Self::unsupported(Some(request.context.id), ResourceErrorPhase::Connect)
    }

    fn prefetch(&self, request: PrefetchRequest) -> ResourceFuture<'_, PrefetchReceipt> {
        Self::unsupported(
            Some(request.request.context.id),
            ResourceErrorPhase::Prefetch,
        )
    }

    fn cancel_request(&self, request_id: RequestId) -> ResourceFuture<'_, ()> {
        Self::unsupported(Some(request_id), ResourceErrorPhase::Cancel)
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
        let mut template =
            lynx_template_decoder::decode(&bytes).map_err(|source| CliError::Decode {
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
            fetcher = fetcher.with_style_sheet(url, sheet);
        }
        let config = PageConfig {
            default_display_linear: template.config_flag("defaultDisplayLinear"),
            default_overflow_visible: template.config_flag("defaultOverflowVisible"),
            enable_css_selector: template.config_flag("enableCSSSelector"),
        };
        Ok(Self {
            input: input.to_string(),
            script_url,
            scoped_css_ids,
            resource_fetcher: Arc::new(fetcher),
            config,
        })
    }

    /// Reports CSS fragments whose rules will apply more widely than the
    /// bundle intended.
    ///
    /// A bundle compiled with `enableRemoveCSSScope = false` keeps one
    /// fragment per component and expects each fragment's rules to match only
    /// inside that component. Per-component scoping is not implemented, so
    /// those rules mount globally and two components that style the same class
    /// name will collide. Rendering them is still better than rendering
    /// nothing, but it is not silent.
    pub(crate) fn warn_about_unscoped_author_styles(&self) {
        if self.scoped_css_ids.is_empty() {
            return;
        }
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
}
