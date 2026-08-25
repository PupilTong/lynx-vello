//! Shared test scaffolding: a scriptable `ResourceFetcher` double.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    CacheStatus, HttpRequest, HttpResponse, RequestId, ResolveRequest, ResolvedLocator,
    ResourceCapability, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    ResourceFuture, ResourceLocality, ResourceMetadata, ResourceRequest, ResourceResponse,
    ResourceSource, ResourceTiming, RetryAdvice, StyleSheetPayload, StyleSheetResponse,
};
use bobcat_core::script::ScriptError;
use bobcat_core::{EngineEvent, OffscreenLynxView, PreparsedStyleSheet};
use bytes::Bytes;
use url::Url;

/// Waits for the engine-owned script thread to report its terminal boot event.
pub fn wait_for_script(view: &mut OffscreenLynxView) -> Result<(), ScriptError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => return Ok(()),
                EngineEvent::ScriptRunError(error) => return Err(error),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Which transports the double advertises, and what it hands back.
#[derive(Debug)]
pub struct FetcherDouble {
    pub bytes: Vec<u8>,
    pub capabilities: Vec<ResourceCapability>,
    /// Overrides the resolved URL, so a test can drive the `data:` branch or a host rewrite
    /// without a real network.
    pub resolve_to: Mutex<Option<String>>,
    pub cache_key: Option<String>,
    pub resolves: AtomicUsize,
    pub fetches: AtomicUsize,
    /// When set, stylesheet requests are answered pre-parsed instead of as
    /// CSS text — the arm a bundle-decoding embedder uses.
    pub style_sheet: Option<Arc<PreparsedStyleSheet>>,
    pub style_sheet_fetches: AtomicUsize,
}

impl FetcherDouble {
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            capabilities: vec![ResourceCapability::BufferedResource],
            resolve_to: Mutex::new(None),
            cache_key: None,
            resolves: AtomicUsize::new(0),
            fetches: AtomicUsize::new(0),
            style_sheet: None,
            style_sheet_fetches: AtomicUsize::new(0),
        }
    }

    /// Answers stylesheet requests with a host-decoded sheet.
    #[must_use]
    pub fn with_preparsed_style_sheet(mut self, sheet: PreparsedStyleSheet) -> Self {
        self.style_sheet = Some(Arc::new(sheet));
        self.capabilities
            .push(ResourceCapability::PreparsedStyleSheet);
        self
    }

    pub fn style_sheet_fetch_count(&self) -> usize {
        self.style_sheet_fetches.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn with_capabilities(mut self, capabilities: Vec<ResourceCapability>) -> Self {
        self.capabilities = capabilities;
        self
    }

    #[must_use]
    pub fn resolving_to(self, url: &str) -> Self {
        *self.resolve_to.lock().expect("resolve override") = Some(url.to_owned());
        self
    }

    #[must_use]
    pub fn with_cache_key(mut self, key: &str) -> Self {
        self.cache_key = Some(key.to_owned());
        self
    }

    pub fn fetch_count(&self) -> usize {
        self.fetches.load(Ordering::Relaxed)
    }

    pub fn resolve_count(&self) -> usize {
        self.resolves.load(Ordering::Relaxed)
    }

    fn metadata(&self, resource: ResolvedLocator, id: RequestId) -> ResourceMetadata {
        ResourceMetadata {
            request_id: id,
            resource,
            headers: http::HeaderMap::new(),
            content_length: Some(self.bytes.len() as u64),
            media_type: None,
            source: ResourceSource::Custom,
            cache_status: CacheStatus::Miss,
            timing: ResourceTiming::default(),
        }
    }
}

fn unsupported(phase: ResourceErrorPhase) -> ResourceError {
    ResourceError {
        request_id: None,
        kind: ResourceErrorKind::UnsupportedOperation,
        phase,
        locator: None,
        status: None,
        message: "not advertised by this double".into(),
        retry: RetryAdvice::Never,
    }
}

impl ResourceFetcher for FetcherDouble {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> ResourceFuture<'_, StyleSheetResponse> {
        self.style_sheet_fetches.fetch_add(1, Ordering::Relaxed);
        let Some(sheet) = self.style_sheet.clone() else {
            // No pre-parsed sheet registered, so behave like a host that only
            // moves bytes: run the trait's own default, which is the code path
            // a browser embedder takes in production.
            return bobcat_core::resource::fetch_style_sheet_as_text(self, request);
        };
        let metadata = self.metadata(request.resource.clone(), request.context.id);
        Box::pin(async move {
            Ok(StyleSheetResponse {
                metadata,
                payload: StyleSheetPayload::Preparsed(sheet),
            })
        })
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        self.resolves.fetch_add(1, Ordering::Relaxed);
        let override_url = self.resolve_to.lock().expect("resolve override").clone();
        let cache_key = self.cache_key.clone();
        Box::pin(async move {
            let text = override_url.unwrap_or_else(|| {
                format!(
                    "https://example.test/{}",
                    request.resource.locator.specifier
                )
            });
            let url = Url::parse(&text).map_err(|error| ResourceError {
                request_id: Some(request.context.id),
                kind: ResourceErrorKind::InvalidUrl,
                phase: ResourceErrorPhase::Resolve,
                locator: Some(request.resource.locator.specifier.clone()),
                status: None,
                message: error.to_string().into(),
                retry: RetryAdvice::Never,
            })?;
            Ok(ResolvedLocator {
                resource: request.resource,
                url,
                rewrite_chain: Vec::new(),
                locality: ResourceLocality::Remote,
                cache_key: cache_key.map(Into::into),
            })
        })
    }

    fn fetch_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceResponse> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        let id = request.context.id;
        let resource = request.resource;
        Box::pin(async move {
            Ok(ResourceResponse {
                metadata: self.metadata(resource, id),
                bytes: Bytes::from(self.bytes.clone()),
            })
        })
    }

    fn fetch_http(&self, _request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        Box::pin(async { Err(unsupported(ResourceErrorPhase::SendRequest)) })
    }
}
