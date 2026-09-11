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
//!   the main thread's `Image` element in the browser, each asked to downsample during decode
//!   ([`decode`]).
//! - **Draw-sized decoding.** The frame reads each image with the size it draws it at
//!   ([`bobcat_core::ImageSizeHint`]); a bitmap far larger than its draw is re-decoded at the drawn
//!   size in the background, so a photo shown as a thumbnail costs a thumbnail.
//!
//! # Shape
//!
//! [`Resources`] is the shared system: registry, caches, job executor,
//! decoder. An embedder builds one, registers what it already holds, and
//! hands [`Resources::builder`] to [`bobcat_core::LynxGroup::create_lynx_view`], which
//! turns it into the per-view [`ViewResources`] that implements the
//! protocol and carries that view's [`ImageReports`]. The painter's thread
//! asks for a load and services what came back; the load itself is one task
//! on the crate's own tokio runtime, whose blocking pool runs the transport,
//! the preprocessing and the platform decoder (in the browser, a local task
//! on the Render Worker instead). Images wake the painter to service
//! reports; source completions send directly to main through the concrete
//! handle supplied with each request.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(clippy::undocumented_unsafe_blocks)]

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobcat_core::resource::ResourceFetcher;
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
mod sources;
pub mod transport;

use crate::executor::Wakeup;
use crate::images::{Completion, ImageState};
use crate::registry::{Registered, Registry};
use crate::transport::{Fetched, HttpSettings, Transports};

/// What a load may do with the caches on its way.
///
/// The fetcher's own vocabulary: every load this crate starts names one, the
/// disk tier turns it into an RFC 9111 plan (`cache::http::plan`), and the
/// browser transport maps it onto a `fetch` request's cache mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CachePolicy {
    /// Ordinary freshness rules: a fresh stored response answers, a stale one
    /// is revalidated.
    #[default]
    Default,
    /// Fetch, and store nothing.
    NoStore,
    /// Fetch unconditionally, ignoring what is stored.
    Reload,
    /// Revalidate whatever is stored before using it.
    NoCache,
    /// Use a stored response however stale, and fetch only without one.
    ForceCache,
    /// Use a stored response however stale, and fail without one.
    OnlyIfCached,
}

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
    /// How many blocking threads the fetcher's runtime may have, natively:
    /// the cap on transport reads, preprocessing and decodes together. `0`
    /// clamps to 1. Its sibling is [`Self::decode_parallelism`].
    pub worker_threads: usize,
    /// How many decodes may be outstanding at once, natively; `None` is one
    /// permit per blocking thread.
    ///
    /// A permit is taken before a decode closure is submitted and held until
    /// it returns, so this bounds queued decodes as well as running ones. At
    /// the default it rarely binds and never below what
    /// [`Self::worker_threads`] threads could decode anyway, though a decode
    /// can still wait behind another job's queued transport read. Lowering it
    /// trades decode concurrency for a longer decode queue and never blocks a
    /// transport read, because no blocking closure ever waits on a permit.
    /// The pool's size stays [`Self::worker_threads`] either way.
    pub decode_parallelism: Option<usize>,
    /// Whether image failures are also printed to standard error, natively.
    pub log_to_stderr: bool,
    /// This Worker's end of the channel whose other end the host's
    /// main-thread image decoder listens on (`js/image-decoder.ts` in the
    /// `bobcat-wasm` package), in the browser. Without it no image decodes.
    #[cfg(target_arch = "wasm32")]
    pub image_port: Option<web_sys::MessagePort>,
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
            decode_parallelism: None,
            log_to_stderr: cfg!(not(target_arch = "wasm32")),
            #[cfg(target_arch = "wasm32")]
            image_port: None,
        }
    }
}

/// The handle jobs hold on [`Shared`]: atomic natively, where a job's task
/// and its blocking closures share it across threads, and plain in the
/// browser, where nothing does and the decoder it holds is thread-bound
/// anyway.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type SharedHandle = Arc<Shared>;
#[cfg(target_arch = "wasm32")]
pub(crate) type SharedHandle = Rc<Shared>;

/// The part of the system every job may touch. It holds no piece of the
/// [`executor::Executor`], so no job can keep the fetcher's runtime alive.
pub(crate) struct Shared {
    transports: Transports,
    base_url: Mutex<Option<Url>>,
    initial_decode_bound: u32,
    downsample_ratio: f32,
    #[cfg(target_arch = "wasm32")]
    decoder: Option<Rc<decode::browser::ImageDecoder>>,
    completions: tokio::sync::mpsc::UnboundedSender<Completion>,
    wakeup: Wakeup,
    log_to_stderr: bool,
    notes: Mutex<Vec<String>>,
    /// A test's seam into a background job's transport read, called on the
    /// blocking thread that runs it, so a test can hold a job in flight or
    /// make one panic. The painter's inline restore never consults it.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    fetch_hook: Mutex<Option<FetchHook>>,
    /// A test's stand-in for the platform decoder, consulted only by the
    /// permit-holding decode of a background job — never by the painter's
    /// inline restore, which is what keeps that decode uncounted.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    decode_hook: Mutex<Option<DecodeHook>>,
}

/// What a test runs on the blocking thread of a background transport read.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) type FetchHook = Arc<dyn Fn(&Url) + Send + Sync>;

/// What a test puts in the place of the platform decoder.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) type DecodeHook = Arc<
    dyn Fn(
            &[u8],
            mime::ImageFormat,
            Option<image_header::ImageHeader>,
            (u32, u32),
        ) -> Result<decode::Bitmap, decode::DecodeError>
        + Send
        + Sync,
>;

impl Shared {
    fn complete(&self, completion: Completion) {
        let _ = self.completions.send(completion);
        (self.wakeup)();
    }

    /// The transport read a background job runs, on a blocking-pool thread.
    #[cfg(not(target_arch = "wasm32"))]
    fn fetch_job(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &http::HeaderMap,
    ) -> Result<Fetched, error::Failure> {
        #[cfg(test)]
        {
            let hook = self
                .fetch_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                hook(url);
            }
        }
        self.transports.fetch_blocking(url, policy, headers)
    }

    /// The decode a background job runs while holding its permit.
    #[cfg(not(target_arch = "wasm32"))]
    fn decode_job(
        &self,
        bytes: &[u8],
        format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        #[cfg(test)]
        {
            let hook = self
                .decode_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                return hook(bytes, format, header, max);
            }
        }
        self.decode_bytes(bytes, format, header, max)
    }

    /// Puts `hook` in the place of the platform decoder for background jobs.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn set_decode_hook(&self, hook: DecodeHook) {
        *self
            .decode_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    /// Calls `hook` on the blocking thread of every background transport read.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn set_fetch_hook(&self, hook: FetchHook) {
        *self
            .fetch_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    /// Decodes `bytes`, which preprocessing found to be a `format` image
    /// with `header`, at most `max` large. The native decoders take the
    /// header; the browser's takes the media type instead.
    #[cfg(not(target_arch = "wasm32"))]
    #[expect(
        clippy::unused_self,
        reason = "the browser variant decodes through a port this holds; one call shape serves both"
    )]
    fn decode_bytes(
        &self,
        bytes: &[u8],
        _format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        decode::decode(bytes, header, max)
    }

    #[cfg(target_arch = "wasm32")]
    fn decode_bytes(
        &self,
        bytes: &[u8],
        format: mime::ImageFormat,
        _header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        self.image_decoder()?
            .decode_blocking(bytes, format.media_type(), max)
    }

    #[cfg(target_arch = "wasm32")]
    async fn decode_bytes_async(
        &self,
        bytes: &[u8],
        format: mime::ImageFormat,
        _header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        self.image_decoder()?
            .decode(bytes, format.media_type(), max)
            .await
    }

    #[cfg(target_arch = "wasm32")]
    fn image_decoder(&self) -> Result<&decode::browser::ImageDecoder, decode::DecodeError> {
        self.decoder.as_deref().ok_or_else(|| {
            decode::DecodeError::Unavailable("no image decoder was configured".to_owned())
        })
    }
}

/// The shared resource system: registry, caches, job executor and decoder.
///
/// Cheap to clone — every clone is the same system — and bound to the
/// thread that built it, which must be the thread that builds the views it
/// serves (the painter's). It is also the only holder of the native
/// executor: when the last clone of the last scope goes, the fetcher's
/// runtime is shut down, on this thread.
#[derive(Clone)]
pub struct Resources {
    shared: SharedHandle,
    /// Shared by every scope, held by nothing a job can reach.
    #[cfg(not(target_arch = "wasm32"))]
    executor: Arc<executor::Executor>,
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
    /// Builds the system. `wakeup` is called whenever a load completes between
    /// painter turns — natively from the executor's driver thread, in the
    /// browser from the Render Worker's own task, there being no driver there.
    /// Pass the same wakeup the view was given, so the completion is answered
    /// by a turn.
    ///
    /// Natively that wakeup must not block, and must not take a lock the
    /// thread dropping the last [`Resources`] can be holding: the drop joins
    /// the driver, so it waits for a wakeup already running.
    ///
    /// Never fails on anything the embedder chose: a disk cache that cannot
    /// be opened, or a decoder that cannot be reached, is recorded in
    /// [`Resources::take_notes`] and the system runs without it.
    ///
    /// # Panics
    ///
    /// Natively, if the platform refuses the fetcher's runtime or its driver
    /// thread. That is the one way this call can end the process, and it
    /// asks the platform for nothing an embedder could have configured
    /// differently.
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
                Ok(cache) => Some(Arc::new(cache)),
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
        let decoder = if let Some(port) = config.image_port.clone() {
            decode::browser::ImageDecoder::new(port)
                .inspect_err(|error| notes.push(format!("images will not decode: {error}")))
                .ok()
        } else {
            notes.push("images will not decode: no image decode port was configured".to_owned());
            None
        };
        let (completions, receiver) = tokio::sync::mpsc::unbounded_channel();
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
            #[cfg(target_arch = "wasm32")]
            decoder,
            completions,
            wakeup: Arc::new(wakeup),
            log_to_stderr: config.log_to_stderr,
            notes: Mutex::new(notes),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            fetch_hook: Mutex::new(None),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            decode_hook: Mutex::new(None),
        });
        Self {
            shared,
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::new(executor::Executor::new(
                config.worker_threads,
                config.decode_parallelism,
            )),
            local: Rc::new(RefCell::new(ImageState::new(
                config.memory_budget_bytes,
                receiver,
            ))),
        }
    }

    /// An independent page resource scope sharing only the executor, the
    /// platform decoder and the disk cache. Registrations, relative URL base,
    /// decoded images and completion queues start empty. Late image results
    /// from a retired page cannot populate the replacement page's cache.
    ///
    /// The executor is shared rather than rebuilt, so a scope costs no
    /// threads and the runtime lives until the last scope of the last clone
    /// is dropped.
    #[must_use]
    pub fn new_scope(&self) -> Self {
        let (completions, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            shared: SharedHandle::new(Shared {
                transports: Transports {
                    registry: Registry::default(),
                    http: self.shared.transports.http.clone(),
                    #[cfg(not(target_arch = "wasm32"))]
                    disk: self.shared.transports.disk.clone(),
                },
                base_url: Mutex::new(None),
                initial_decode_bound: self.shared.initial_decode_bound,
                downsample_ratio: self.shared.downsample_ratio,
                #[cfg(target_arch = "wasm32")]
                decoder: self.shared.decoder.clone(),
                completions,
                wakeup: self.shared.wakeup.clone(),
                log_to_stderr: self.shared.log_to_stderr,
                notes: Mutex::new(Vec::new()),
                #[cfg(all(test, not(target_arch = "wasm32")))]
                fetch_hook: Mutex::new(None),
                #[cfg(all(test, not(target_arch = "wasm32")))]
                decode_hook: Mutex::new(None),
            }),
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::clone(&self.executor),
            local: Rc::new(RefCell::new(ImageState::new(
                self.local.borrow().budget(),
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

    /// Registers a stylesheet the host already parsed. It answers a stylesheet
    /// source request pre-parsed, and no other request at all — it has no
    /// bytes to give one.
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

    /// The builder [`bobcat_core::LynxGroup::create_lynx_view`] takes.
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

impl ResourceFetcher for ViewResources {
    fn request_source(
        &self,
        request: bobcat_core::resource::SourceRequest,
        completion: bobcat_core::resource::SourceCompletion,
    ) {
        sources::request(&self.resources, request, completion);
    }

    fn request_image(&self, source: &str) {
        images::request(&self.resources, source, &self.reports);
    }

    fn service_images(&self) {
        images::service(&self.resources);
    }
}

impl FrameImages for ViewResources {
    fn read(&self, source: &str, hint: ImageSizeHint) -> Option<ImageData> {
        images::read(&self.resources, source, hint)
    }

    fn retain(&self, frame: &[Arc<str>]) {
        images::retain(&self.resources, frame);
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn replacement_scope_isolates_registrations_and_late_image_completions() {
        let old = Resources::new(
            ResourcesConfig {
                worker_threads: 1,
                log_to_stderr: false,
                ..ResourcesConfig::default()
            },
            || {},
        );
        let url = "bobcat-memory://archive/image.png";
        old.register(url, vec![1], None).unwrap();
        old.set_base_url(Some(Url::parse(url).unwrap()));
        let replacement = old.new_scope();
        assert!(replacement.base_url().is_none());
        assert!(!replacement.unregister(url));
        replacement.register(url, vec![2], None).unwrap();
        old.shared.complete(Completion::Failed {
            source: Arc::from("image.png"),
            message: "retired page".to_owned(),
        });
        images::service(&replacement);
        assert!(!replacement.knows_image("image.png"));
        assert!(replacement.take_notes().is_empty());
        assert!(old.unregister(url));
        assert!(replacement.unregister(url));
    }
}
