//! `bobcat-resources` — the cross-platform reference resource system for
//! Bobcat embedders: one [`ResourceFetcher`] for macOS, Linux and the
//! browser.
//!
//! [`bobcat_core`] deliberately owns no byte of a resource: its
//! [`ResourceFetcher`] protocol names what a host must answer and leaves
//! every transport, cache and codec to the host. This crate is the answer a
//! host can take as it is, or read as the worked example of what the
//! protocol expects:
//!
//! - **Transports.** Contents the embedder [registers](Resources::register) under any URL, `data:`
//!   URLs, `file:` URLs on native targets, and `http(s)` through the platform's own client —
//!   libcurl loaded at runtime on macOS and Linux, `fetch` in the browser's Render Worker. The
//!   crate links no HTTP or TLS stack.
//! - **A MIME-keyed preprocessing pipeline.** Every fetched payload is sniffed ([`mime::sniff`]),
//!   classified, and treated by class: text is transcoded to UTF-8 with its BOM removed, JSON is
//!   validated, images are container-sniffed and header-probed for the size layout needs, and the
//!   rest passes through ([`preprocess`]).
//! - **Tiered caching.** Decoded bitmaps live in a memory tier under a byte budget with the frame's
//!   working set pinned ([`cache::memory`]); fetched bytes live on disk under their own budget with
//!   HTTP freshness and revalidation semantics ([`cache::disk`], [`cache::http`]), natively; the
//!   browser's HTTP cache plays that role there.
//! - **Platform image decoding.** No codec is compiled in: `ImageIO` on macOS, gdk-pixbuf on Linux,
//!   `createImageBitmap` in the browser, each asked to downsample during decode ([`decode`]).
//! - **Draw-sized decoding.** The frame reads each image with the size it draws it at
//!   ([`bobcat_core::ImageSizeHint`]); a bitmap far larger than its draw is re-decoded at the drawn
//!   size in the background, so a photo shown as a thumbnail costs a thumbnail.
//!
//! # Shape
//!
//! [`Resources`] is the shared system: registry, caches, worker pool,
//! decoder. An embedder builds one, registers what it already holds, and
//! hands [`Resources::builder`] to [`bobcat_core::LynxView::new`], which
//! turns it into the per-view [`ViewResources`] that implements the
//! protocol and carries that view's [`ImageReports`]. Loads complete on
//! worker threads (or as browser tasks) and are delivered through the
//! wakeup the embedder supplies at construction; the painter's next turn
//! applies them.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(clippy::undocumented_unsafe_blocks)]

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobcat_core::resource::{
    ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError, ResourceFetcher,
    ResourceMetadata, ResourceRequest, ResourceResponse, ResourceSource, ResourceTiming,
    StyleSheetPayload, StyleSheetResponse, fetch_style_sheet_as_text,
};
use bobcat_core::vello::peniko::ImageData;
use bobcat_core::{FrameImages, ImageReports, ImageSizeHint, PreparsedStyleSheet};
use bytes::Bytes;
use url::Url;

pub mod cache;
pub mod data_url;
pub mod decode;
mod error;
mod executor;
pub mod image_header;
mod images;
pub mod mime;
pub mod preprocess;
mod registry;
pub mod transport;

pub use crate::executor::Wakeup;
use crate::images::{Completion, ImageState};
use crate::registry::{Registered, Registry};
use crate::transport::{Fetched, HttpSettings, Transports};

/// The disk tier's location and size.
#[derive(Clone, Debug)]
pub struct DiskCacheConfig {
    pub dir: std::path::PathBuf,
    pub budget_bytes: u64,
}

impl DiskCacheConfig {
    /// The platform's per-user cache directory for Bobcat, or `None` when
    /// the environment does not name one.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn at_default_location(budget_bytes: u64) -> Option<Self> {
        cache::disk::default_cache_dir().map(|dir| Self { dir, budget_bytes })
    }
}

/// Everything a [`Resources`] is built with. `Default` is a working
/// configuration for a desktop host with no disk cache.
#[derive(Clone, Debug)]
pub struct ResourcesConfig {
    /// What relative specifiers resolve against — the page's own URL.
    pub base_url: Option<Url>,
    /// The memory tier's budget for decoded bitmaps. Best-effort: the frame
    /// being drawn is never evicted, and a single bitmap larger than the
    /// budget still has to exist while it is on screen.
    pub memory_budget_bytes: usize,
    /// The disk tier; `None` keeps everything in memory.
    pub disk_cache: Option<DiskCacheConfig>,
    /// The largest per-axis size of an image's first decode, before any
    /// frame has said how large it draws it. Also bounded by what vello can
    /// draw at all.
    pub initial_decode_bound: u32,
    /// A resident bitmap at least this many times larger than its draw on
    /// both axes is re-decoded at the drawn size.
    pub downsample_ratio: f32,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
    pub user_agent: String,
    pub max_redirects: u32,
    /// IO and decode worker threads, natively.
    pub worker_threads: usize,
    /// Whether image failures are also printed to standard error, natively.
    pub log_to_stderr: bool,
    /// The URL of the image decode worker script (`image-worker.js` in the
    /// `bobcat-wasm` package), in the browser. Without it no image decodes.
    #[cfg(target_arch = "wasm32")]
    pub image_worker_url: Option<String>,
}

impl Default for ResourcesConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            memory_budget_bytes: 64 * 1024 * 1024,
            disk_cache: None,
            initial_decode_bound: 2048,
            downsample_ratio: 2.0,
            request_timeout: Duration::from_secs(30),
            max_response_bytes: 64 * 1024 * 1024,
            user_agent: concat!("bobcat-resources/", env!("CARGO_PKG_VERSION")).to_owned(),
            max_redirects: 10,
            worker_threads: std::thread::available_parallelism()
                .map_or(2, |count| count.get().clamp(1, 4)),
            log_to_stderr: cfg!(not(target_arch = "wasm32")),
            #[cfg(target_arch = "wasm32")]
            image_worker_url: None,
        }
    }
}

/// The handle jobs hold on [`Shared`]: atomic natively, where worker
/// threads share it, and plain in the browser, where nothing does and the
/// decoder it holds is thread-bound anyway.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type SharedHandle = Arc<Shared>;
#[cfg(target_arch = "wasm32")]
pub(crate) type SharedHandle = Rc<Shared>;

/// The part of the system every job may touch.
pub(crate) struct Shared {
    transports: Transports,
    base_url: Mutex<Option<Url>>,
    initial_decode_bound: u32,
    downsample_ratio: f32,
    #[cfg(not(target_arch = "wasm32"))]
    executor: executor::Executor,
    #[cfg(target_arch = "wasm32")]
    decoder: Option<Rc<decode::browser::ImageWorker>>,
    completions: flume::Sender<Completion>,
    wakeup: Wakeup,
    log_to_stderr: bool,
    notes: Mutex<Vec<String>>,
}

impl Shared {
    fn complete(&self, completion: Completion) {
        let _ = self.completions.send(completion);
        (self.wakeup)();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[expect(
        clippy::unused_self,
        reason = "the browser variant decodes through a worker this holds; one call shape serves both"
    )]
    fn decode_bytes(
        &self,
        bytes: &[u8],
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        decode::decode(bytes, header, max)
    }

    #[cfg(target_arch = "wasm32")]
    fn decode_bytes(
        &self,
        bytes: &[u8],
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        self.decoder
            .as_ref()
            .ok_or_else(|| {
                decode::DecodeError::Unavailable("no image worker was configured".to_owned())
            })?
            .decode_blocking(bytes, header, max)
    }

    #[cfg(target_arch = "wasm32")]
    async fn decode_bytes_async(
        &self,
        bytes: &[u8],
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        let decoder = self.decoder.as_ref().ok_or_else(|| {
            decode::DecodeError::Unavailable("no image worker was configured".to_owned())
        })?;
        decoder.decode(bytes, header, max).await
    }
}

/// The shared resource system: registry, caches, workers and decoder.
///
/// Cheap to clone — every clone is the same system — and bound to the
/// thread that built it, which must be the thread that builds the views it
/// serves (the painter's).
#[derive(Clone)]
pub struct Resources {
    shared: SharedHandle,
    local: Rc<RefCell<ImageState>>,
}

impl fmt::Debug for Resources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Resources")
            .field("transports", &self.shared.transports)
            .finish_non_exhaustive()
    }
}

/// A URL could not be registered.
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("`{url}` is not a valid URL: {message}")]
    InvalidUrl { url: String, message: String },
}

impl Resources {
    /// Builds the system. `wakeup` is called from a worker whenever a load
    /// completes between painter turns — pass the same wakeup the view was
    /// given, so the completion is answered by a turn.
    ///
    /// Never fails: a disk cache that cannot be opened, or a decoder that
    /// cannot be reached, is recorded in [`Resources::take_notes`] and the
    /// system runs without it.
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "a configuration is handed over to the system it configures"
    )]
    pub fn new(config: ResourcesConfig, wakeup: impl Fn() + Send + Sync + 'static) -> Self {
        let mut notes = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        let disk = config.disk_cache.as_ref().and_then(|disk| {
            match cache::disk::DiskCache::open(&disk.dir, disk.budget_bytes) {
                Ok(cache) => Some(cache),
                Err(error) => {
                    notes.push(format!(
                        "the disk cache at `{}` could not be opened: {error}",
                        disk.dir.display()
                    ));
                    None
                }
            }
        });
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(error) = decode::available() {
            notes.push(format!("images will not decode: {error}"));
        }
        #[cfg(target_arch = "wasm32")]
        let decoder = if let Some(url) = config.image_worker_url.as_deref() {
            decode::browser::ImageWorker::new(url)
                .inspect_err(|error| notes.push(format!("images will not decode: {error}")))
                .ok()
        } else {
            notes.push("images will not decode: no image worker URL was configured".to_owned());
            None
        };
        let (completions, receiver) = flume::unbounded();
        let shared = SharedHandle::new(Shared {
            transports: Transports {
                registry: Registry::default(),
                http: HttpSettings {
                    timeout: config.request_timeout,
                    max_body: config.max_response_bytes,
                    user_agent: config.user_agent.clone(),
                    max_redirects: config.max_redirects,
                },
                #[cfg(not(target_arch = "wasm32"))]
                disk,
            },
            base_url: Mutex::new(config.base_url.clone()),
            initial_decode_bound: config.initial_decode_bound.max(1),
            downsample_ratio: config.downsample_ratio.max(1.0),
            #[cfg(not(target_arch = "wasm32"))]
            executor: executor::Executor::new(config.worker_threads),
            #[cfg(target_arch = "wasm32")]
            decoder,
            completions,
            wakeup: Arc::new(wakeup),
            log_to_stderr: config.log_to_stderr,
            notes: Mutex::new(notes),
        });
        Self {
            shared,
            local: Rc::new(RefCell::new(ImageState::new(
                config.memory_budget_bytes,
                receiver,
            ))),
        }
    }

    /// Registers `bytes` under `url`, replacing any earlier registration.
    /// `media_type` labels them the way a `Content-Type` would; without it
    /// they are sniffed. Returns the normalized URL they answer to.
    pub fn register(
        &self,
        url: &str,
        bytes: impl Into<Bytes>,
        media_type: Option<&str>,
    ) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.shared.transports.registry.insert(
            &url,
            Registered::Bytes {
                bytes: bytes.into(),
                media_type: media_type.and_then(mime::MediaType::parse),
            },
        );
        Ok(url)
    }

    /// Registers a stylesheet the host already parsed. It answers
    /// `fetch_style_sheet` pre-parsed and nothing else.
    pub fn register_style_sheet(
        &self,
        url: &str,
        sheet: PreparsedStyleSheet,
    ) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.shared
            .transports
            .registry
            .insert(&url, Registered::StyleSheet(Arc::new(sheet)));
        Ok(url)
    }

    /// Forgets a registration. Images already decoded from it stay decoded.
    #[must_use = "the answer says whether anything was registered under the URL"]
    pub fn unregister(&self, url: &str) -> bool {
        parse_registration_url(url)
            .ok()
            .and_then(|url| self.shared.transports.registry.remove(&url))
            .is_some()
    }

    /// Forgets every registration.
    pub fn clear_registered(&self) {
        self.shared.transports.registry.clear();
    }

    /// What relative specifiers resolve against.
    #[must_use]
    pub fn base_url(&self) -> Option<Url> {
        self.shared
            .base_url
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_base_url(&self, base_url: Option<Url>) {
        *self
            .shared
            .base_url
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = base_url;
    }

    /// Bytes the memory tier holds: decoded bitmaps, plus the encoded
    /// images nothing else could restore.
    #[must_use]
    pub fn memory_used_bytes(&self) -> usize {
        let state = self.local.borrow();
        state.bitmap_bytes() + state.encoded_bytes()
    }

    /// Changes the bitmap budget, evicting what no longer fits.
    pub fn set_memory_budget_bytes(&self, budget: usize) {
        self.local.borrow_mut().set_budget(budget);
    }

    /// Whether `source` currently has decoded pixels in memory.
    #[must_use]
    pub fn is_resident(&self, source: &str) -> bool {
        self.local.borrow().is_resident(source)
    }

    /// The resident bitmap's size for `source`, if it has one.
    #[must_use]
    pub fn resident_size(&self, source: &str) -> Option<(u32, u32)> {
        self.local.borrow().resident_size(source)
    }

    /// Whether `source` has been asked for.
    #[must_use]
    pub fn knows_image(&self, source: &str) -> bool {
        self.local.borrow().knows(source)
    }

    /// Diagnostics recorded since the last call: an image that failed, a
    /// cache that could not be opened, a decoder that is missing.
    #[must_use]
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .shared
                .notes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// The per-view value for one [`bobcat_core::LynxView`], carrying its
    /// [`ImageReports`].
    #[must_use]
    pub fn for_view(&self, reports: ImageReports) -> ViewResources {
        ViewResources {
            resources: self.clone(),
            reports,
        }
    }

    /// The builder [`bobcat_core::LynxView::new`] takes.
    pub fn builder(&self) -> impl FnOnce(ImageReports) -> ViewResources + 'static {
        let resources = self.clone();
        move |reports| resources.for_view(reports)
    }

    pub(crate) fn note(&self, message: String) {
        if self.shared.log_to_stderr {
            eprintln!("bobcat-resources: {message}");
        }
        self.shared
            .notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    /// Fetches and preprocesses `url` off the painter's thread.
    #[cfg(not(target_arch = "wasm32"))]
    async fn fetch(
        &self,
        url: Url,
        policy: bobcat_core::resource::CachePolicy,
        headers: http::HeaderMap,
    ) -> Result<(Fetched, preprocess::Preprocessed), error::Failure> {
        let (sender, receiver) = flume::bounded(1);
        let shared = SharedHandle::clone(&self.shared);
        self.shared.executor.run(move || {
            let result = shared
                .transports
                .fetch_blocking(&url, policy, &headers)
                .and_then(|fetched| preprocess_fetched(fetched, &url));
            let _ = sender.send(result);
        });
        receiver.recv_async().await.unwrap_or_else(|_| {
            Err(error::Failure::new(
                bobcat_core::resource::ResourceErrorKind::Unavailable,
                bobcat_core::resource::ResourceErrorPhase::Open,
                "the resource worker went away before answering",
            ))
        })
    }

    /// Fetches and preprocesses `url` through the browser.
    #[cfg(target_arch = "wasm32")]
    async fn fetch(
        &self,
        url: Url,
        policy: bobcat_core::resource::CachePolicy,
        headers: http::HeaderMap,
    ) -> Result<(Fetched, preprocess::Preprocessed), error::Failure> {
        let fetched = self.shared.transports.fetch(&url, policy, &headers).await?;
        preprocess_fetched(fetched, &url)
    }
}

fn preprocess_fetched(
    fetched: Fetched,
    url: &Url,
) -> Result<(Fetched, preprocess::Preprocessed), error::Failure> {
    let preprocessed = preprocess::preprocess(
        fetched.bytes.clone(),
        fetched.media_type.as_ref(),
        Some(url),
    )
    .map_err(|error| {
        error::Failure::new(
            bobcat_core::resource::ResourceErrorKind::ResponseBody,
            bobcat_core::resource::ResourceErrorPhase::ReadBody,
            error.to_string(),
        )
    })?;
    Ok((fetched, preprocessed))
}

fn parse_registration_url(url: &str) -> Result<Url, RegisterError> {
    Url::parse(url).map_err(|error| RegisterError::InvalidUrl {
        url: url.to_owned(),
        message: error.to_string(),
    })
}

/// One view's end of the system: the [`ResourceFetcher`] it is built with.
pub struct ViewResources {
    resources: Resources,
    reports: ImageReports,
}

impl fmt::Debug for ViewResources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ViewResources")
            .field("resources", &self.resources)
            .finish_non_exhaustive()
    }
}

impl ViewResources {
    /// The shared system behind this view.
    #[must_use]
    pub fn resources(&self) -> &Resources {
        &self.resources
    }
}

/// The response metadata for `request`, answered by `fetched`.
fn metadata(
    request: &ResourceRequest,
    fetched: &Fetched,
    media_type: &mime::MediaType,
    content_length: Option<u64>,
) -> ResourceMetadata {
    let mut resource = request.resource.clone();
    if fetched.url != resource.url {
        resource
            .rewrite_chain
            .extend(fetched.redirects.iter().cloned());
        resource.url = fetched.url.clone();
    }
    ResourceMetadata {
        request_id: request.context.id,
        resource,
        headers: fetched.headers.clone(),
        content_length,
        media_type: Some(Arc::from(media_type.to_string())),
        source: fetched.source,
        cache_status: fetched.cache_status,
        timing: fetched.timing,
    }
}

impl ResourceFetcher for ViewResources {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        matches!(
            capability,
            ResourceCapability::BufferedResource | ResourceCapability::PreparsedStyleSheet
        )
    }

    async fn resolve_locator(
        &self,
        request: ResolveRequest,
    ) -> Result<ResolvedLocator, ResourceError> {
        let specifier = request.resource.specifier.clone();
        let base = request
            .resource
            .base_url
            .clone()
            .or_else(|| self.resources.base_url());
        let url = self
            .resources
            .shared
            .transports
            .resolve(&specifier, base.as_ref())
            .map_err(|failure| failure.into_error(Some(request.context.id), Some(specifier)))?;
        let locality = Transports::locality(&url);
        Ok(ResolvedLocator {
            resource: request.resource,
            cache_key: Some(Arc::from(url.as_str())),
            url,
            rewrite_chain: Vec::new(),
            locality,
        })
    }

    async fn fetch_resource(
        &self,
        request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError> {
        let locator: Arc<str> = Arc::from(request.resource.url.as_str());
        let (fetched, preprocessed) = self
            .resources
            .fetch(
                request.resource.url.clone(),
                request.cache_policy,
                request.headers.clone(),
            )
            .await
            .map_err(|failure| failure.into_error(Some(request.context.id), Some(locator)))?;
        let content_length = Some(preprocessed.bytes.len() as u64);
        Ok(ResourceResponse {
            metadata: metadata(&request, &fetched, &preprocessed.media_type, content_length),
            bytes: preprocessed.bytes,
        })
    }

    async fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> Result<StyleSheetResponse, ResourceError> {
        if let Some(Registered::StyleSheet(sheet)) = self
            .resources
            .shared
            .transports
            .registry
            .get(&request.resource.url)
        {
            let media_type = mime::MediaType::parse("text/css").expect("a media type");
            let fetched = Fetched {
                bytes: Bytes::new(),
                media_type: Some(media_type.clone()),
                url: request.resource.url.clone(),
                redirects: Vec::new(),
                source: ResourceSource::PackagedAsset,
                cache_status: bobcat_core::resource::CacheStatus::NotApplicable,
                headers: http::HeaderMap::new(),
                timing: ResourceTiming::default(),
                restorable: false,
            };
            return Ok(StyleSheetResponse {
                metadata: metadata(&request, &fetched, &media_type, None),
                payload: StyleSheetPayload::Preparsed(sheet),
            });
        }
        fetch_style_sheet_as_text(self, request).await
    }

    fn request_image(&self, source: &str) {
        images::request(&self.resources, source, &self.reports);
    }

    fn retain_images(&self, frame: &[Arc<str>]) {
        images::retain(&self.resources, frame);
    }

    fn service_images(&self) {
        images::service(&self.resources);
    }
}

impl FrameImages for ViewResources {
    fn read(&self, source: &str, hint: ImageSizeHint) -> Option<ImageData> {
        images::read(&self.resources, source, hint)
    }
}
