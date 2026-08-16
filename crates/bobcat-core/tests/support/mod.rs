//! Shared test scaffolding: an in-process PNG encoder and a scriptable
//! `ResourceFetcher` double.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bobcat_core::PreparsedStyleSheet;
use bobcat_core::resource::{
    BufferedResourceRequest, CacheStatus, CancellationToken, HttpRequest, HttpResponse,
    PrefetchReceipt, PrefetchRequest, RequestId, ResolveRequest, ResolvedLocator,
    ResourceCapability, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    ResourceFuture, ResourceLocality, ResourceMetadata, ResourcePath, ResourcePathLease,
    ResourceRequest, ResourceResponse, ResourceSource, ResourceStream, ResourceTiming, RetryAdvice,
    StyleSheetPayload, StyleSheetResponse,
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

/// A 4x4 image whose four quadrants are red, green, blue and transparent — enough that a flipped,
/// rotated or channel-swapped decode is visible.
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

/// A minimal PNG-only [`Decoder`], injected into the contract tests exactly the way an embedder
/// injects a real one.
#[derive(Debug)]
pub struct PngDouble;

use bobcat_core::image::{
    Acceleration, AlphaType, Capabilities, DecodeRequest, DecodeResponse, DecodedImage, Decoder,
    ImageError, ImageFormat, ImageHeader, PixelSize,
};

impl Decoder for PngDouble {
    fn name(&self) -> &'static str {
        "png-double"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::none().with(ImageFormat::Png, Acceleration::Software)
    }

    fn probe(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
        if format != ImageFormat::Png {
            return Err(ImageError::Unsupported { format });
        }
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = decoder
            .read_info()
            .map_err(|error| ImageError::decode(format, error.to_string()))?;
        let info = reader.info();
        Ok(ImageHeader {
            format,
            natural_size: PixelSize {
                width: info.width,
                height: info.height,
            },
            has_alpha: info.color_type.samples() % 2 == 0 || info.trns.is_some(),
            animated: info.animation_control.is_some(),
        })
    }

    fn decode(
        &self,
        format: ImageFormat,
        bytes: &[u8],
        request: &DecodeRequest,
    ) -> Result<DecodeResponse, ImageError> {
        let header = self.probe(format, bytes)?;
        request.check(&header)?;

        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|error| ImageError::decode(format, error.to_string()))?;
        let mut pixels =
            vec![
                0u8;
                reader
                    .output_buffer_size()
                    .ok_or_else(|| ImageError::decode(format, "output size overflows"))?
            ];
        let frame = reader
            .next_frame(&mut pixels)
            .map_err(|error| ImageError::decode(format, error.to_string()))?;
        pixels.truncate(frame.buffer_size());
        if reader.output_color_type().0 != png::ColorType::Rgba {
            return Err(ImageError::decode(format, "the double decodes RGBA8 only"));
        }

        let target = request.effective_size(header.natural_size);
        let pixels = if target == header.natural_size {
            pixels
        } else {
            resample_nearest(
                &pixels,
                (frame.width, frame.height),
                (target.width, target.height),
            )
        };

        Ok(DecodeResponse {
            image: DecodedImage::from_rgba8(
                target.width,
                target.height,
                AlphaType::Straight,
                pixels,
                format,
            )?,
            header,
            acceleration: Acceleration::Software,
            backend: "png-double",
        })
    }
}

fn resample_nearest(pixels: &[u8], from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity((to.0 * to.1 * 4) as usize);
    for y in 0..to.1 {
        let sy = y * from.1 / to.1;
        for x in 0..to.0 {
            let sx = x * from.0 / to.0;
            let at = ((sy * from.0 + sx) * 4) as usize;
            out.extend_from_slice(&pixels[at..at + 4]);
        }
    }
    out
}

/// The injected decoder every contract test uses.
#[must_use]
pub fn decoder() -> Arc<dyn Decoder> {
    Arc::new(PngDouble)
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
    /// Makes `resolve_locator` never complete, standing in for embedder code blocked on a network
    /// round trip or a lock.
    pub hang_resolve: bool,
    pub resolves: AtomicUsize,
    pub fetches: AtomicUsize,
    pub prefetches: AtomicUsize,
    pub cancels: AtomicUsize,
    pub observed_cancellation: Mutex<Option<CancellationToken>>,
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
            hang_resolve: false,
            resolves: AtomicUsize::new(0),
            fetches: AtomicUsize::new(0),
            prefetches: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            observed_cancellation: Mutex::new(None),
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
    pub fn with_hung_resolve(mut self) -> Self {
        self.hang_resolve = true;
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

    pub fn request_cancellation(&self) -> Option<CancellationToken> {
        self.observed_cancellation
            .lock()
            .expect("observed cancellation")
            .clone()
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

#[derive(Debug)]
struct TempPathLease(PathBuf);

impl ResourcePathLease for TempPathLease {}

impl Drop for TempPathLease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl ResourceFetcher for FetcherDouble {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    fn fetch_style_sheet(
        &self,
        request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, StyleSheetResponse> {
        self.style_sheet_fetches.fetch_add(1, Ordering::Relaxed);
        let Some(sheet) = self.style_sheet.clone() else {
            let bytes = Bytes::from(self.bytes.clone());
            let metadata =
                self.metadata(request.request.resource.clone(), request.request.context.id);
            return Box::pin(async move {
                Ok(StyleSheetResponse {
                    metadata,
                    payload: StyleSheetPayload::Text(bytes),
                })
            });
        };
        let metadata = self.metadata(request.request.resource.clone(), request.request.context.id);
        Box::pin(async move {
            Ok(StyleSheetResponse {
                metadata,
                payload: StyleSheetPayload::Preparsed(sheet),
            })
        })
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        self.resolves.fetch_add(1, Ordering::Relaxed);
        *self
            .observed_cancellation
            .lock()
            .expect("observed cancellation") = Some(request.context.cancellation.clone());
        let override_url = self.resolve_to.lock().expect("resolve override").clone();
        let cache_key = self.cache_key.clone();
        let hang = self.hang_resolve;
        Box::pin(async move {
            if hang {
                std::future::pending::<()>().await;
            }
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
                "lynx-vello-image-fixture-{self:p}-{}-{}.bin",
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
            let lease = Arc::new(TempPathLease(path.clone()));
            Ok(ResourcePath {
                metadata: self.metadata(resource, id),
                path,
                fallback_paths: Vec::new(),
                lease: Some(lease),
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
