//! Loading a `.web.bundle` into the plain values the engine consumes.
//!
//! This is embedder IO: read the bytes, decode the template, retain its root
//! script behind the resource-fetching contract, and hand its URL plus page
//! config to Bobcat. The pipeline itself — tree, commits, style, layout,
//! paint, scheduling — remains the engine's.

use std::sync::Arc;

use bobcat_core::PageConfig;
use bobcat_core::resource::{
    BufferedResourceRequest, CacheStatus, HttpRequest, HttpResponse, PrefetchReceipt,
    PrefetchRequest, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourceLocality,
    ResourceMetadata, ResourcePath, ResourceRequest, ResourceResponse, ResourceSource,
    ResourceStream, ResourceTiming, RetryAdvice,
};
use http::HeaderMap;
use url::Url;

use crate::CliError;

#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) input: String,
    pub(crate) script_url: Url,
    pub(crate) resource_fetcher: Arc<ProgramResourceFetcher>,
    pub(crate) config: PageConfig,
    author_rule_count: usize,
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
}

impl ProgramResourceFetcher {
    fn new(script_url: Url, source: String) -> Self {
        Self {
            script_url,
            source: Arc::from(source),
        }
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
        capability == ResourceCapability::BufferedResource
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
        if url != self.script_url {
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
        let resource_fetcher = Arc::new(ProgramResourceFetcher::new(script_url.clone(), source));
        let config = PageConfig {
            default_display_linear: template.config_flag("defaultDisplayLinear"),
            default_overflow_visible: template.config_flag("defaultOverflowVisible"),
            enable_css_selector: template.config_flag("enableCSSSelector"),
        };
        let author_rule_count = template.style_info.as_ref().map_or(0, |style_info| {
            style_info
                .css_id_to_style_sheet
                .values()
                .map(|sheet| sheet.rules.len())
                .sum()
        });
        Ok(Self {
            input: input.to_string(),
            script_url,
            resource_fetcher,
            config,
            author_rule_count,
        })
    }

    pub(crate) fn warn_about_dropped_author_rules(&self) {
        if self.author_rule_count != 0 {
            eprintln!(
                "bobcat: warning: {} contains {} decoded author rule(s), but StyleInfo ingestion \
                 is not implemented yet; author styles are omitted",
                self.input, self.author_rule_count
            );
        }
    }
}
