//! Shared test scaffolding: a scriptable `ResourceFetcher` double.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    CacheStatus, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourceLocality,
    ResourceMetadata, ResourceRequest, ResourceResponse, ResourceSource, ResourceTiming,
    RetryAdvice, StyleSheetPayload, StyleSheetResponse,
};
use bobcat_core::script::ScriptError;
use bobcat_core::{EngineEvent, LynxView, PreparsedStyleSheet};
use bytes::Bytes;
use url::Url;

/// Drains the terminal boot event preserved after construction. `LynxView::new`
/// has already awaited the same outcome before it returns, and every `pump`
/// here runs the view's own turn on this thread.
pub fn wait_for_script(view: &mut LynxView) -> Result<(), ScriptError> {
    // Generous, like the engine's own BEGIN_FRAME_TIMEOUT: a debug-build
    // boot takes about two seconds on its own, so a tight deadline only
    // ever fires spuriously under parallel test load.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => return Ok(()),
                EngineEvent::ScriptRunError(error) => return Err(error),
                // Not a script failure, but a view that cannot draw will
                // never finish anything either; failing here beats waiting
                // out the deadline.
                EngineEvent::RenderFailed(error) => panic!("the painter failed: {error}"),
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
    /// When set, stylesheet requests use these bytes while ordinary resource
    /// requests keep using `bytes` for the entry module.
    pub style_sheet_text: Option<Vec<u8>>,
    pub style_sheet_fetches: AtomicUsize,
    /// The images this host serves, if a test installed any. Shared with the
    /// test through an `Arc` so it can publish pixels and read the retain log
    /// while the painter owns its own handle.
    pub images: Option<Arc<flashbulb::TestImages>>,
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
            style_sheet_text: None,
            style_sheet_fetches: AtomicUsize::new(0),
            images: None,
        }
    }

    /// Serves images from `images`, which the test keeps a handle on so it
    /// can publish pixels and read the retain log.
    #[must_use]
    pub fn with_images(mut self, images: Arc<flashbulb::TestImages>) -> Self {
        self.images = Some(images);
        self
    }

    /// This double as the one thing a view is built from: a factory that
    /// hands the painter the whole resource system, images included.
    #[must_use]
    pub fn into_factory(self) -> bobcat_core::ResourceFactory {
        Box::new(move |sink| {
            if let Some(images) = self.images.as_ref() {
                images.attach(sink);
            }
            std::rc::Rc::new(self)
        })
    }

    /// Answers stylesheet requests with a host-decoded sheet.
    #[must_use]
    pub fn with_preparsed_style_sheet(mut self, sheet: PreparsedStyleSheet) -> Self {
        self.style_sheet = Some(Arc::new(sheet));
        self.capabilities
            .push(ResourceCapability::PreparsedStyleSheet);
        self
    }

    /// Answers stylesheet requests with raw text bytes independently of the
    /// entry-module bytes returned by `fetch_resource`.
    #[must_use]
    pub fn with_style_sheet_text(mut self, bytes: Vec<u8>) -> Self {
        self.style_sheet_text = Some(bytes);
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
        if let Some(sheet) = self.style_sheet.clone() {
            let metadata = self.metadata(request.resource.clone(), request.context.id);
            return Box::pin(async move {
                Ok(StyleSheetResponse {
                    metadata,
                    payload: StyleSheetPayload::Preparsed(sheet),
                })
            });
        }
        if let Some(bytes) = self.style_sheet_text.clone() {
            let mut metadata = self.metadata(request.resource, request.context.id);
            metadata.content_length = Some(bytes.len() as u64);
            return Box::pin(async move {
                Ok(StyleSheetResponse {
                    metadata,
                    payload: StyleSheetPayload::Text(Bytes::from(bytes)),
                })
            });
        }
        // No dedicated sheet registered, so behave like a host that only
        // moves bytes: run the trait's own default.
        bobcat_core::resource::fetch_style_sheet_as_text(self, request)
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        self.resolves.fetch_add(1, Ordering::Relaxed);
        let override_url = self.resolve_to.lock().expect("resolve override").clone();
        let cache_key = self.cache_key.clone();
        Box::pin(async move {
            let text = override_url
                .unwrap_or_else(|| format!("https://example.test/{}", request.resource.specifier));
            let url = Url::parse(&text).map_err(|error| ResourceError {
                request_id: Some(request.context.id),
                kind: ResourceErrorKind::InvalidUrl,
                phase: ResourceErrorPhase::Resolve,
                locator: Some(request.resource.specifier.clone()),
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

    fn request_image(&self, source: &str) {
        if let Some(images) = self.images.as_ref() {
            images.request(source);
        }
    }

    fn retain_images(&self, frame: &[Arc<str>]) {
        if let Some(images) = self.images.as_ref() {
            images.retain(frame);
        }
    }
}

/// A double serves images only when a test gave it a store; otherwise every
/// image draw resolves to nothing, which is what an unloaded image looks like.
impl bobcat_core::FrameImages for FetcherDouble {
    fn read(&self, source: &str) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.images.as_ref().and_then(|images| images.read(source))
    }
}

/// A fetcher the painter owns while the test keeps a handle on it.
///
/// The painter's handle is an `Rc`, because a fetcher never leaves that
/// thread; a test still has to inspect the double afterwards, and two of them
/// assert on when it is *dropped*, which needs the `Arc` to stay weakly
/// observable.
pub struct SharedFetcher<F>(pub Arc<F>);

impl<F: bobcat_core::FrameImages> bobcat_core::FrameImages for SharedFetcher<F> {
    fn read(&self, source: &str) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.0.read(source)
    }
}

impl<F: ResourceFetcher> ResourceFetcher for SharedFetcher<F> {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.0.supports_capability(capability)
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        self.0.resolve_locator(request)
    }

    fn fetch_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceResponse> {
        self.0.fetch_resource(request)
    }

    fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> ResourceFuture<'_, StyleSheetResponse> {
        self.0.fetch_style_sheet(request)
    }

    fn request_image(&self, source: &str) {
        self.0.request_image(source);
    }

    fn retain_images(&self, frame: &[Arc<str>]) {
        self.0.retain_images(frame);
    }
}

/// The one thing a view is built from, over a double the test still holds.
#[must_use]
pub fn factory<F: ResourceFetcher + 'static>(fetcher: Arc<F>) -> bobcat_core::ResourceFactory {
    Box::new(move |_sink| std::rc::Rc::new(SharedFetcher(fetcher)))
}

/// [`factory`], with the view's image sink wired into a test image store, so
/// completed loads reach the document the way a real host's would.
#[must_use]
pub fn factory_serving_images<F: ResourceFetcher + 'static>(
    fetcher: Arc<F>,
    images: Arc<flashbulb::TestImages>,
) -> bobcat_core::ResourceFactory {
    Box::new(move |sink| {
        images.attach(sink);
        std::rc::Rc::new(SharedFetcher(fetcher))
    })
}
