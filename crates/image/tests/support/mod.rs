//! Shared test scaffolding: an in-process PNG encoder and a scriptable
//! `ResourceFetcher` double.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use bobcat_engine::resource::{
    BufferedResourceRequest, CacheStatus, HttpRequest, HttpResponse, PrefetchReceipt,
    PrefetchRequest, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourceLocality,
    ResourceMetadata, ResourcePath, ResourceRequest, ResourceResponse, ResourceSource,
    ResourceStream, ResourceTiming, RetryAdvice,
};
use bytes::Bytes;
use url::Url;

/// Encodes an RGBA8 buffer as a PNG, so PNG fixtures never need committing.
#[must_use]
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(rgba).expect("png data");
    }
    bytes
}

/// A 4x4 image whose four quadrants are red, green, blue and transparent —
/// enough that a flipped, rotated or channel-swapped decode is visible.
#[must_use]
pub fn checker_rgba(side: u32) -> Vec<u8> {
    let half = side / 2;
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let pixel = match (x < half, y < half) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [0, 0, 0, 0],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}

#[must_use]
pub fn checker_png(side: u32) -> Vec<u8> {
    encode_png(side, side, &checker_rgba(side))
}

#[must_use]
pub fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Which transports the double advertises, and what it hands back.
#[derive(Debug)]
pub struct FetcherDouble {
    pub bytes: Vec<u8>,
    pub capabilities: Vec<ResourceCapability>,
    /// Overrides the resolved URL, so a test can drive the `data:` branch or a
    /// host rewrite without a real network.
    pub resolve_to: Mutex<Option<String>>,
    pub cache_key: Option<String>,
    pub resolves: AtomicUsize,
    pub fetches: AtomicUsize,
    pub prefetches: AtomicUsize,
    pub cancels: AtomicUsize,
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
            prefetches: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
        }
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

    fn fetch_resource(
        &self,
        request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, ResourceResponse> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        let id = request.request.context.id;
        let resource = request.request.resource;
        Box::pin(async move {
            Ok(ResourceResponse {
                metadata: self.metadata(resource, id),
                bytes: Bytes::from(self.bytes.clone()),
            })
        })
    }

    fn open_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceStream> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        let id = request.context.id;
        let resource = request.resource;
        Box::pin(async move {
            Ok(ResourceStream {
                metadata: self.metadata(resource, id),
                reader: Box::pin(std::io::Cursor::new(self.bytes.clone())),
            })
        })
    }

    fn fetch_resource_path(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourcePath> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        let id = request.context.id;
        let resource = request.resource;
        Box::pin(async move {
            let path = std::env::temp_dir().join(format!(
                "lynx-vello-image-fixture-{}-{}.bin",
                id.namespace, id.sequence
            ));
            std::fs::write(&path, &self.bytes).map_err(|error| ResourceError {
                request_id: Some(id),
                kind: ResourceErrorKind::Io,
                phase: ResourceErrorPhase::MaterializePath,
                locator: None,
                status: None,
                message: error.to_string().into(),
                retry: RetryAdvice::Never,
            })?;
            Ok(ResourcePath {
                metadata: self.metadata(resource, id),
                path,
                fallback_paths: Vec::new(),
                lease: None,
            })
        })
    }

    fn fetch_http(&self, _request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        Box::pin(async { Err(unsupported(ResourceErrorPhase::SendRequest)) })
    }

    fn prefetch(&self, request: PrefetchRequest) -> ResourceFuture<'_, PrefetchReceipt> {
        self.prefetches.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            Ok(PrefetchReceipt {
                request_id: request.request.context.id,
                resource: request.request.resource,
                cache_status: CacheStatus::Miss,
                transferred_bytes: Some(self.bytes.len() as u64),
            })
        })
    }

    fn cancel_request(&self, _request_id: RequestId) -> ResourceFuture<'_, ()> {
        self.cancels.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(()) })
    }
}
