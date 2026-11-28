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
//! - **Tiered caching, applied from `vant`.** The cache policy is `vant`'s and this crate applies
//!   it: decoded bitmaps live in a memory tier under a byte budget with the frame's working set
//!   pinned ([`cache::memory`]); fetched bytes live in `vant`'s byte tier
//!   ([`vant::byte_cache::ByteCache`]) under their own budget with HTTP freshness and revalidation
//!   semantics ([`cache::http`]), over the platform's [`Store`](vant::Store) — natively a cache
//!   directory ([`FsStore`]) when the embedder configures one. A transport that applies HTTP cache
//!   semantics itself, the browser's `fetch`, gets no byte tier: its HTTP cache plays that role.
//!   Stylesheet responses and pending loads are shared by URL within each resource scope, including
//!   preload hints.
//! - **The code cache's Store side.** The engine looks compiled script bytecode up in, and files
//!   what it compiled into, `vant`'s process-wide artifact tier
//!   ([`vant::artifact::Artifacts::global`]). When the platform hands over an artifact store —
//!   natively the `code` directory inside the cache directory, in the browser a cache of the web
//!   Cache API — this crate preloads the stored bytecode of a script's text before it delivers that
//!   text, and writes what the engine compiled back once per embedder turn
//!   ([`vant::artifact::ArtifactCache`] over a synchronous store,
//!   [`vant::artifact::AsyncArtifactCache`] over an asynchronous one).
//! - **Platform image decoding.** No codec is compiled in: `ImageIO` on macOS, gdk-pixbuf on Linux,
//!   the Render Worker's `createImageBitmap` in the browser, each asked to downsample during decode
//!   ([`decode`]). A decoder may answer frames of its own type rather than CPU pixels
//!   ([`vant::ImageDecoder::Frame`]): the browser's `ImageBitmap`s are held and released by the
//!   memory tier as CPU pixels are, and reach the painter as platform frames it has the frame type
//!   write into a texture ([`bobcat_core::FrameWriter`]).
//! - **SVG documents parsed for the engine.** An SVG is not decoded to pixels: on every target, the
//!   browser included, it is parsed where a bitmap would be decoded, under a decode permit, with
//!   the engine's own parser ([`bobcat_core::ImageEvent::parse_document`]), and reported through
//!   [`ImageReports::parsed_document`]; the engine draws the document itself. Markup a page wrote
//!   (`<svg content>`) arrives as a document request with its bytes and is parsed the same way. A
//!   document holds no bitmap, so the memory tier and draw-sized decoding do not apply, and it is
//!   never retained: every request is parsed and reported, and the engine owns what it encodes.
//! - **Animated images.** An animated GIF, APNG or WebP plays wherever a URL picture is drawn, for
//!   the loop count its file states, on one playhead per URL shared by every draw
//!   (`vant::image::animation`); its frames are decoded by index a bounded window ahead of the
//!   playhead where the platform decoder can (`ImageIO`, the browser's `WebCodecs` `ImageDecoder`),
//!   and a timer on this crate's own executor wakes the host when the next one is due. gdk-pixbuf
//!   answers one frame, and so does a browser without `ImageDecoder`.
//! - **Videos.** A `video` element's picture is filled by the platform's own player
//!   ([`vant::media::Media`]): `AVFoundation` on macOS, the page's `<video>` in the browser
//!   (`bobcat-wasm`), none on Linux, where every open fails. A source the player cannot read itself
//!   is fetched here and handed over as bytes; the player publishes each frame into the picture's
//!   slot while it is drawn, and its events reach the view with the image reports ([`media`]).
//! - **Draw-sized decoding.** The frame reads each image with the size it draws it at
//!   ([`bobcat_core::ImageSizeHint`]); a bitmap far larger than its draw is re-decoded at the drawn
//!   size in the background, so a photo shown as a thumbnail costs a thumbnail.
//!
//! # Shape
//!
//! This crate is the orchestration — [`Resources`], its job executor, the
//! image and source pipelines, the registry — and the reference adapters for
//! the three capabilities `vant` asks of an embedder, bundled as a
//! [`vant::Platform`]: [`NativePlatform`] ([`NativeFetch`], [`FsStore`],
//! [`NativeDecoder`]) on macOS and Linux. It is the native
//! [`DefaultPlatform`], which [`Resources::new`] builds with no persistent
//! store; an embedder with a disk cache, its own HTTP stack or its own
//! storage builds a platform and hands it to [`Resources::with_platform`].
//! The browser's platform is `bobcat-wasm`'s, so on wasm32 there is no
//! default and every [`Resources`] names its platform.
//!
//! [`Resources`] is the shared system: registry, caches, job executor,
//! decoder. An embedder builds one, registers what it already holds, and
//! hands [`Resources::builder`] to [`bobcat_core::LynxGroup::create_lynx_view`], which
//! turns it into the per-view [`ViewResources`] that implements the
//! protocol and carries that view's [`ImageReports`]. The painter's thread
//! asks for a load and services what came back; the load itself is one task
//! on the crate's own tokio runtime, whose blocking pool runs the transport,
//! the preprocessing and then, for a raster image, the platform decoder, or
//! for an SVG document the engine's parser (in the browser, a local task on
//! the Render Worker instead). A load therefore ends as a decoded bitmap, a
//! parsed document, or a failure. Images wake the painter to service
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
use bobcat_core::{
    ClockInstant, FrameImages, ImageReports, ImageResidency, ImageSizeHint, PictureId, PictureKey,
    PreparsedStyleSheet,
};
use bytes::Bytes;
use rustc_hash::FxHashSet;
use url::Url;

mod animation;
pub mod cache;
pub mod decode;
mod executor;
#[cfg(not(target_arch = "wasm32"))]
pub mod file_body;
mod images;
pub mod media;
#[cfg(not(target_arch = "wasm32"))]
pub mod platform;
mod registry;
mod sources;
pub mod store;
pub mod transport;

#[cfg(not(target_arch = "wasm32"))]
pub use decode::NativeDecoder;
#[cfg(not(target_arch = "wasm32"))]
pub use platform::NativePlatform;
#[cfg(not(target_arch = "wasm32"))]
pub use store::FsStore;
#[cfg(not(target_arch = "wasm32"))]
pub use transport::NativeFetch;
use vant::artifact::{ArtifactCache, ArtifactConfig, Artifacts, AsyncArtifactCache, SourceDigest};
use vant::byte_cache::ByteCache;
use vant::image::residency::ViewKey;
pub use vant::registry::{ContainerInstaller, RegisterError, Registered};
use vant::registry::{Register, RegisterInBackground, parse_registration_url};
use vant::{ArtifactStore, AsyncStore, Parts, Platform, Store};
pub use vant::{CachePolicy, data_url, image_header, mime, preprocess};

use crate::executor::Wakeup;
use crate::images::{Completion, ImageState};
use crate::transport::{Fetched, HttpSettings, Transports};

pub type Registrar = vant::registry::Registrar<PreparsedStyleSheet>;

pub(crate) type Registry = vant::registry::Registry<PreparsedStyleSheet>;

#[cfg(not(target_arch = "wasm32"))]
pub type DefaultPlatform = platform::NativePlatform;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) const CODE_CACHE_DIR: &str = "code";

#[derive(Clone, Debug)]
pub struct DiskCacheConfig {
    pub dir: std::path::PathBuf,
    pub budget_bytes: u64,
    pub code_budget_bytes: u64,
}

impl DiskCacheConfig {
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn at_default_location(budget_bytes: u64) -> Option<Self> {
        store::default_cache_dir().map(|dir| Self {
            dir,
            budget_bytes,
            code_budget_bytes: ArtifactConfig::DEFAULT_BUDGET_BYTES,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ResourcesConfig {
    pub base_url: Option<Url>,
    pub memory_budget_bytes: usize,
    pub initial_decode_bound: u32,
    pub downsample_ratio: f32,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
    pub user_agent: String,
    pub max_redirects: u32,
    pub worker_threads: usize,
    pub decode_parallelism: Option<usize>,
    pub log_to_stderr: bool,
    pub container_installer: Option<Arc<dyn ContainerInstaller<PreparsedStyleSheet>>>,
}

impl Default for ResourcesConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            memory_budget_bytes: 64 * 1024 * 1024,
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
            container_installer: None,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct FetchIndex {
    base_url: Mutex<Option<Url>>,
    fetched: Mutex<FxHashSet<Url>>,
}

impl FetchIndex {
    fn with_base(base_url: Option<Url>) -> Self {
        Self {
            base_url: Mutex::new(base_url),
            fetched: Mutex::default(),
        }
    }

    fn base_url(&self) -> Option<Url> {
        self.base_url
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_base_url(&self, base_url: Option<Url>) {
        *self
            .base_url
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = base_url;
    }

    pub(crate) fn remember(&self, url: &Url) {
        self.fetched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(url.clone());
    }

    fn contains(&self, specifier: &str) -> bool {
        let Some(url) = self.resolve(specifier) else {
            return false;
        };
        self.fetched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&url)
    }

    fn resolve(&self, specifier: &str) -> Option<Url> {
        transport::join(specifier, self.base_url().as_ref()).ok()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Handle<T> = Arc<T>;
#[cfg(target_arch = "wasm32")]
pub(crate) type Handle<T> = Rc<T>;

pub(crate) type SharedHandle<P> = Handle<Shared<P>>;

pub type FrameOf<P> = <<P as Platform>::Decoder as vant::ImageDecoder>::Frame;

pub(crate) struct CodeCache<S: Store, A: AsyncStore> {
    memory: Arc<Artifacts>,
    driver: CodeDriver<S, A>,
    flushing: std::sync::atomic::AtomicBool,
}

enum CodeDriver<S: Store, A: AsyncStore> {
    Blocking(ArtifactCache<S>),
    Async {
        store: Mutex<Option<A>>,
        budget: u64,
        cache: tokio::sync::OnceCell<AsyncArtifactCache<A>>,
    },
}

fn code_config(budget: u64) -> ArtifactConfig {
    ArtifactConfig {
        budget_bytes: budget,
        ..ArtifactConfig::default()
    }
}

impl<S: Store, A: AsyncStore> CodeCache<S, A> {
    fn open(store: ArtifactStore<S, A>, budget: u64) -> Self {
        let memory = Arc::clone(Artifacts::global());
        let driver = match store {
            ArtifactStore::Blocking(store) => CodeDriver::Blocking(ArtifactCache::open(
                Arc::clone(&memory),
                store,
                bobcat_core::script_engine_id(),
                code_config(budget),
                transport::now(),
            )),
            ArtifactStore::Async(store) => CodeDriver::Async {
                store: Mutex::new(Some(store)),
                budget,
                cache: tokio::sync::OnceCell::new(),
            },
        };
        Self {
            memory,
            driver,
            flushing: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn is_async(&self) -> bool {
        matches!(self.driver, CodeDriver::Async { .. })
    }

    async fn opened(&self) -> Option<&AsyncArtifactCache<A>> {
        let CodeDriver::Async {
            store,
            budget,
            cache,
        } = &self.driver
        else {
            return None;
        };
        cache
            .get_or_try_init(|| async {
                let store = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .ok_or(())?;
                Ok::<_, ()>(
                    AsyncArtifactCache::open(
                        Arc::clone(&self.memory),
                        store,
                        bobcat_core::script_engine_id(),
                        code_config(*budget),
                        transport::now(),
                    )
                    .await,
                )
            })
            .await
            .ok()
    }

    pub(crate) fn preload(&self, text: &str) {
        if let CodeDriver::Blocking(cache) = &self.driver
            && text.len() >= bobcat_core::MIN_CACHED_SOURCE_BYTES
        {
            cache.preload(SourceDigest::of(text.as_bytes()), transport::now());
        }
    }

    pub(crate) async fn preload_async(&self, text: &str) {
        if !self.is_async() || text.len() < bobcat_core::MIN_CACHED_SOURCE_BYTES {
            return;
        }
        if let Some(cache) = self.opened().await {
            cache
                .preload(SourceDigest::of(text.as_bytes()), transport::now())
                .await;
        }
    }
}

struct Flushing<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for Flushing<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

impl<S: Store, A: AsyncStore> Drop for CodeCache<S, A> {
    fn drop(&mut self) {
        if let CodeDriver::Blocking(cache) = &self.driver {
            cache.flush(transport::now());
        }
    }
}

impl<S: Store, A: AsyncStore> fmt::Debug for CodeCache<S, A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("CodeCache");
        match &self.driver {
            CodeDriver::Blocking(cache) => debug.field("cache", cache),
            CodeDriver::Async { cache, .. } => debug.field("cache", &cache.get()),
        };
        debug.finish_non_exhaustive()
    }
}

pub(crate) struct Shared<P: Platform> {
    transports: Transports<P>,
    code: Option<Handle<CodeCache<P::Store, P::AsyncStore>>>,
    fetches: Arc<FetchIndex>,
    initial_decode_bound: u32,
    downsample_ratio: f32,
    decoder: Handle<P::Decoder>,
    media: Handle<P::Media>,
    completions: tokio::sync::mpsc::UnboundedSender<Completion<FrameOf<P>>>,
    wakeup: Wakeup,
    log_to_stderr: bool,
    container_installer: Option<Arc<dyn ContainerInstaller<PreparsedStyleSheet>>>,
    notes: Mutex<Vec<String>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    fetch_hook: Mutex<Option<FetchHook>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    decode_hook: Mutex<Option<DecodeHook>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    clock_hook: Mutex<Option<ClockHook>>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) type ClockHook = Arc<dyn Fn() -> ClockInstant + Send + Sync>;

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) type FetchHook = Arc<dyn Fn(&Url) + Send + Sync>;

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

impl<P: Platform> Shared<P> {
    fn install_container(&self, url: &Url, bytes: &[u8]) -> Result<bool, String> {
        let Some(installer) = self.container_installer.as_ref() else {
            return Ok(false);
        };
        installer.install(
            url,
            bytes,
            &Registrar::new(Arc::clone(&self.transports.registry)),
        )
    }

    fn note(&self, message: String) {
        if self.log_to_stderr {
            eprintln!("bobcat-resources: {message}");
        }
        self.notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    fn complete(&self, completion: Completion<FrameOf<P>>) {
        let _ = self.completions.send(completion);
        (self.wakeup)();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fetch_job(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &http::HeaderMap,
        body: file_body::Body,
    ) -> Result<Fetched, vant::error::Failure> {
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
        self.transports.fetch_blocking(url, policy, headers, body)
    }

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

    #[cfg_attr(
        any(not(test), target_arch = "wasm32"),
        expect(clippy::unused_self, reason = "a test build reads its own clock here")
    )]
    fn now(&self) -> ClockInstant {
        #[cfg(all(test, not(target_arch = "wasm32")))]
        {
            let hook = self
                .clock_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                return hook();
            }
        }
        ClockInstant::now()
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn set_clock_hook(&self, hook: ClockHook) {
        *self
            .clock_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn frames_job(
        &self,
        bytes: &[u8],
        format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
    ) -> Option<vant::image::animation::ImageFrames> {
        if !format.may_animate() {
            return None;
        }
        vant::ImageDecoder::frames_blocking(&*self.decoder, bytes, format, header)
            .ok()
            .filter(vant::image::animation::ImageFrames::is_animated)
    }

    #[cfg(target_arch = "wasm32")]
    async fn frames_async(
        &self,
        bytes: Bytes,
        format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
    ) -> Option<vant::image::animation::ImageFrames> {
        if !format.may_animate() {
            return None;
        }
        vant::ImageDecoder::frames(&*self.decoder, bytes, format, header)
            .await
            .ok()
            .filter(vant::image::animation::ImageFrames::is_animated)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn decode_frame_job(
        &self,
        decode: &animation::FrameDecode,
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        vant::ImageDecoder::decode_frame_blocking(
            &*self.decoder,
            &decode.bytes,
            decode.format,
            decode.header,
            decode.index,
            decode.max,
        )
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn set_decode_hook(&self, hook: DecodeHook) {
        *self
            .decode_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn set_fetch_hook(&self, hook: FetchHook) {
        *self
            .fetch_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn decode_bytes(
        &self,
        bytes: &[u8],
        format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        vant::ImageDecoder::decode_blocking(&*self.decoder, bytes, format, header, max)
    }

    #[cfg(target_arch = "wasm32")]
    async fn decode_bytes_async(
        &self,
        bytes: Bytes,
        format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<vant::image::DecodedImage<FrameOf<P>>, decode::DecodeError> {
        vant::ImageDecoder::decode(&*self.decoder, bytes, format, header, max).await
    }
}

pub struct Resources<
    #[cfg(not(target_arch = "wasm32"))] P: Platform = DefaultPlatform,
    #[cfg(target_arch = "wasm32")] P: Platform,
> {
    shared: SharedHandle<P>,
    #[cfg(not(target_arch = "wasm32"))]
    executor: Arc<executor::Executor>,
    local: Rc<RefCell<ImageState<FrameOf<P>>>>,
    media: Rc<RefCell<media::MediaPictures<P>>>,
    style_cache: Rc<RefCell<sources::StyleCache>>,
}

impl<P: Platform> Clone for Resources<P> {
    fn clone(&self) -> Self {
        Self {
            shared: SharedHandle::clone(&self.shared),
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::clone(&self.executor),
            local: Rc::clone(&self.local),
            media: Rc::clone(&self.media),
            style_cache: Rc::clone(&self.style_cache),
        }
    }
}

impl<P: Platform> fmt::Debug for Resources<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Resources")
            .field("transports", &self.shared.transports)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Resources<DefaultPlatform> {
    #[must_use]
    pub fn new(config: ResourcesConfig, wakeup: impl Fn() + Send + Sync + 'static) -> Self {
        let (platform, notes) = DefaultPlatform::new(None);
        Self::with_platform(config, platform, notes, wakeup)
    }
}

impl<P: Platform> Resources<P> {
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "a configuration is handed over to the system it configures"
    )]
    pub fn with_platform(
        config: ResourcesConfig,
        platform: P,
        notes: Vec<String>,
        wakeup: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let Parts {
            fetch,
            decoder,
            media,
            bytes,
            artifacts,
        } = platform.into_parts();
        let bytes = bytes
            .filter(|_| !vant::Fetch::owns_http_cache(&fetch))
            .map(|tier| Handle::new(ByteCache::open(tier.store, tier.budget)));
        let code = artifacts.map(|tier| Handle::new(CodeCache::open(tier.store, tier.budget)));
        let (completions, receiver) = tokio::sync::mpsc::unbounded_channel();
        let initial_decode_bound = config.initial_decode_bound.max(1);
        let downsample_ratio = config.downsample_ratio.max(1.0);
        let shared = SharedHandle::new(Shared {
            transports: Transports::new(
                HttpSettings {
                    timeout: config.request_timeout,
                    max_body: config.max_response_bytes,
                    user_agent: config.user_agent.clone(),
                    max_redirects: config.max_redirects,
                },
                Handle::new(fetch),
                bytes,
            ),
            code,
            fetches: Arc::new(FetchIndex::with_base(config.base_url.clone())),
            initial_decode_bound,
            downsample_ratio,
            decoder: Handle::new(decoder),
            media: Handle::new(media),
            completions,
            wakeup: Arc::new(wakeup),
            log_to_stderr: config.log_to_stderr,
            container_installer: config.container_installer.clone(),
            notes: Mutex::new(notes),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            fetch_hook: Mutex::new(None),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            decode_hook: Mutex::new(None),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            clock_hook: Mutex::new(None),
        });
        let resources = Self {
            shared,
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::new(executor::Executor::new(
                config.worker_threads,
                config.decode_parallelism,
            )),
            style_cache: Rc::default(),
            media: Rc::default(),
            local: Rc::new(RefCell::new(ImageState::new(
                ImageResidency::new(
                    config.memory_budget_bytes,
                    initial_decode_bound,
                    downsample_ratio,
                ),
                ClockInstant::now(),
                receiver,
            ))),
        };
        if let Some(code) = resources
            .shared
            .code
            .as_ref()
            .filter(|code| code.is_async())
        {
            let code = Handle::clone(code);
            resources.spawn(async move {
                code.opened().await;
            });
        }
        resources
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.executor.spawn(task);
    }

    #[cfg(target_arch = "wasm32")]
    #[expect(clippy::unused_self, reason = "the native twin spawns on the executor")]
    fn spawn(&self, task: impl Future<Output = ()> + 'static) {
        wasm_bindgen_futures::spawn_local(task);
    }

    #[must_use]
    pub fn new_scope(&self) -> Self {
        let (completions, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            shared: SharedHandle::new(Shared {
                transports: self.shared.transports.for_new_scope(),
                code: self.shared.code.clone(),
                fetches: Arc::new(FetchIndex::default()),
                initial_decode_bound: self.shared.initial_decode_bound,
                downsample_ratio: self.shared.downsample_ratio,
                decoder: Handle::clone(&self.shared.decoder),
                media: Handle::clone(&self.shared.media),
                completions,
                wakeup: self.shared.wakeup.clone(),
                log_to_stderr: self.shared.log_to_stderr,
                container_installer: self.shared.container_installer.clone(),
                notes: Mutex::new(Vec::new()),
                #[cfg(all(test, not(target_arch = "wasm32")))]
                fetch_hook: Mutex::new(None),
                #[cfg(all(test, not(target_arch = "wasm32")))]
                decode_hook: Mutex::new(None),
                #[cfg(all(test, not(target_arch = "wasm32")))]
                clock_hook: Mutex::new(None),
            }),
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::clone(&self.executor),
            style_cache: Rc::default(),
            media: Rc::default(),
            local: Rc::new(RefCell::new(ImageState::new(
                ImageResidency::new(
                    self.local.borrow().residency.budget(),
                    self.shared.initial_decode_bound,
                    self.shared.downsample_ratio,
                ),
                ClockInstant::now(),
                receiver,
            ))),
        }
    }

    pub fn register(
        &self,
        url: &str,
        bytes: impl Into<Bytes>,
        media_type: Option<&str>,
    ) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.style_cache.borrow_mut().remove(&url);
        self.shared.transports.registry.insert(
            &url,
            Registered::Bytes {
                bytes: bytes.into(),
                media_type: media_type.and_then(mime::MediaType::parse),
            },
        );
        Ok(url)
    }

    pub fn register_style_sheet(
        &self,
        url: &str,
        sheet: Arc<PreparsedStyleSheet>,
    ) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.style_cache.borrow_mut().remove(&url);
        self.shared
            .transports
            .registry
            .insert(&url, Registered::StyleSheet(sheet));
        Ok(url)
    }

    pub fn register_in_background(
        &self,
        container: &Url,
        install: impl FnOnce(&Registrar) -> Result<(), String> + Send + 'static,
    ) {
        static NEXT_INSTALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_INSTALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, done) = tokio::sync::watch::channel(None);
        self.shared.transports.installs.hold(id, container, done);
        let shared = SharedHandle::clone(&self.shared);
        let registrar = Registrar::new(Arc::clone(&shared.transports.registry));
        let finish = move |result: Result<(), String>| {
            if let Err(message) = &result {
                shared.note(format!("a container could not be registered: {message}"));
            }
            shared.transports.installs.release(id);
            let _ = sender.send(Some(result.map_err(Arc::from)));
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let handle = self.executor.handle();
            self.executor.spawn(async move {
                let result =
                    executor::blocking(&handle, "container install", move || install(&registrar))
                        .await;
                finish(result.and_then(|installed| installed));
            });
        }
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move { finish(install(&registrar)) });
    }

    #[must_use = "the answer says whether anything was registered under the URL"]
    pub fn unregister(&self, url: &str) -> bool {
        let Ok(url) = parse_registration_url(url) else {
            return false;
        };
        if self.shared.transports.registry.remove(&url).is_none() {
            return false;
        }
        self.style_cache.borrow_mut().remove(&url);
        true
    }

    pub fn clear_registered(&self) {
        self.style_cache
            .borrow_mut()
            .retain(|url, _| !self.shared.transports.registry.contains(url));
        self.shared.transports.registry.clear();
    }

    #[must_use]
    pub fn base_url(&self) -> Option<Url> {
        self.shared.fetches.base_url()
    }

    pub fn set_base_url(&self, base_url: Option<Url>) {
        self.shared.fetches.set_base_url(base_url);
    }

    #[must_use]
    pub fn memory_used_bytes(&self) -> usize {
        let state = self.local.borrow();
        state.residency.bitmap_bytes()
            + state.residency.encoded_bytes()
            + state.animations.held_bytes()
    }

    pub fn set_memory_budget_bytes(&self, budget: usize) {
        self.local.borrow_mut().residency.set_budget(budget);
    }

    #[must_use]
    pub fn is_resident(&self, source: &str) -> bool {
        self.local.borrow().residency.is_resident(source)
    }

    #[must_use]
    pub fn resident_size(&self, source: &str) -> Option<(u32, u32)> {
        self.local.borrow().residency.resident_size(source)
    }

    #[must_use]
    pub fn knows_image(&self, source: &str) -> bool {
        self.local.borrow().residency.knows(source)
    }

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

    #[must_use]
    pub fn for_view(&self, reports: ImageReports) -> ViewResources<P> {
        let view = self.local.borrow_mut().residency.new_view();
        ViewResources {
            resources: self.clone(),
            reports,
            view,
        }
    }

    pub fn builder(&self) -> impl FnOnce(ImageReports) -> ViewResources<P> + 'static {
        let resources = self.clone();
        move |reports| resources.for_view(reports)
    }

    pub(crate) fn note(&self, message: String) {
        self.shared.note(message);
    }

    pub(crate) fn now(&self) -> ClockInstant {
        self.shared.now()
    }

    pub(crate) fn wake(&self) {
        (self.shared.wakeup)();
    }
}

impl<P: Platform> Register<PreparsedStyleSheet> for Resources<P> {
    fn register(
        &self,
        url: &str,
        bytes: Bytes,
        media_type: Option<&str>,
    ) -> Result<Url, RegisterError> {
        Self::register(self, url, bytes, media_type)
    }

    fn register_style_sheet(
        &self,
        url: &str,
        sheet: Arc<PreparsedStyleSheet>,
    ) -> Result<Url, RegisterError> {
        Self::register_style_sheet(self, url, sheet)
    }
}

impl<P: Platform> RegisterInBackground<PreparsedStyleSheet> for Resources<P> {
    fn register_in_background(
        &self,
        container: &Url,
        install: impl FnOnce(&Registrar) -> Result<(), String> + Send + 'static,
    ) {
        Self::register_in_background(self, container, install);
    }
}

fn flush_code_cache<P: Platform>(resources: &Resources<P>) {
    use std::sync::atomic::Ordering;

    let Some(code) = resources.shared.code.as_ref() else {
        return;
    };
    if !code.memory.has_pending() || code.flushing.swap(true, Ordering::AcqRel) {
        return;
    }
    if code.is_async() {
        let code = Handle::clone(code);
        resources.spawn(async move {
            let _flushing = Flushing(&code.flushing);
            if let Some(cache) = code.opened().await {
                cache.flush(transport::now()).await;
            }
        });
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let shared = SharedHandle::clone(&resources.shared);
        let handle = resources.executor.handle();
        resources.executor.spawn(async move {
            let Some(code) = shared.code.clone() else {
                return;
            };
            let written = executor::blocking(&handle, "code cache flush", {
                let code = Handle::clone(&code);
                move || code.flush_blocking()
            })
            .await;
            code.flushing.store(false, Ordering::Release);
            if let Err(message) = written {
                shared.note(message);
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        code.flush_blocking();
        code.flushing.store(false, Ordering::Release);
    }
}

impl<S: Store, A: AsyncStore> CodeCache<S, A> {
    fn flush_blocking(&self) {
        if let CodeDriver::Blocking(cache) = &self.driver {
            cache.flush(transport::now());
        }
    }
}

fn preprocess_fetched(
    fetched: Fetched,
    url: &Url,
) -> Result<(Fetched, preprocess::Preprocessed), vant::error::Failure> {
    let preprocessed = preprocess::preprocess(
        fetched.bytes.clone(),
        fetched.media_type.as_ref(),
        Some(url),
        mime::SniffContext::Other,
    )
    .map_err(|error| {
        vant::error::Failure::new(
            bobcat_core::resource::ResourceErrorKind::ResponseBody,
            bobcat_core::resource::ResourceErrorPhase::ReadBody,
            error.to_string(),
        )
    })?;
    Ok((fetched, preprocessed))
}

pub struct ViewResources<
    #[cfg(not(target_arch = "wasm32"))] P: Platform = DefaultPlatform,
    #[cfg(target_arch = "wasm32")] P: Platform,
> {
    resources: Resources<P>,
    reports: ImageReports,
    view: ViewKey,
}

impl<P: Platform> Drop for ViewResources<P> {
    fn drop(&mut self) {
        let now = self.resources.now();
        let stopped = if let Ok(mut state) = self.resources.local.try_borrow_mut() {
            state.residency.release_view(self.view);
            state.animations.release_view(self.view, now)
        } else {
            Vec::new()
        };
        images::forget_frames(&self.resources, stopped);
        if let Ok(mut media) = self.resources.media.try_borrow_mut() {
            media.release_view(self.view);
        }
    }
}

impl<P: Platform> fmt::Debug for ViewResources<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ViewResources")
            .field("resources", &self.resources)
            .finish_non_exhaustive()
    }
}

impl<P: Platform> ViewResources<P> {
    #[must_use]
    pub fn resources(&self) -> &Resources<P> {
        &self.resources
    }
}

impl<P: Platform> ResourceFetcher for ViewResources<P>
where
    FrameOf<P>: bobcat_core::FrameWriter,
{
    fn request_source(
        &self,
        request: bobcat_core::resource::SourceRequest,
        completion: bobcat_core::resource::SourceCompletion,
    ) {
        sources::request(&self.resources, request, completion);
    }

    fn preload_source(&self, request: bobcat_core::resource::SourceRequest) {
        sources::preload(&self.resources, request);
    }

    fn request_image(&self, source: &str) {
        images::request(&self.resources, source, &self.reports);
    }

    fn request_document(&self, source: &str, bytes: Bytes, kind: bobcat_core::DocumentKind) {
        images::request_document(&self.resources, source, bytes, kind, &self.reports);
    }

    fn request_playback(&self, request: bobcat_core::PlaybackRequest) {
        images::playback(&self.resources, self.view, &self.reports, request);
    }

    fn service_images(&self) {
        images::service(&self.resources);
        self.resources.media.borrow().service();
        flush_code_cache(&self.resources);
    }

    fn fetch_probe(&self) -> Option<bobcat_core::resource::FetchProbe> {
        let index = Arc::clone(&self.resources.shared.fetches);
        Some(Arc::new(move |specifier: &str| index.contains(specifier)))
    }

    fn style_sheet_probe(&self) -> Option<bobcat_core::resource::StyleSheetProbe> {
        let index = Arc::clone(&self.resources.shared.fetches);
        let registry = Arc::clone(&self.resources.shared.transports.registry);
        Some(Arc::new(move |specifier: &str| {
            index
                .resolve(specifier)
                .is_some_and(|url| registry.contains(&url))
        }))
    }
}

fn urls(pictures: &[PictureKey]) -> Vec<Arc<str>> {
    pictures
        .iter()
        .filter_map(|picture| match picture {
            PictureKey::Url(url) => Some(Arc::clone(url)),
            PictureKey::Id(_) => None,
        })
        .collect()
}

fn ids(pictures: &[PictureKey]) -> Vec<PictureId> {
    pictures.iter().filter_map(PictureKey::id).collect()
}

impl<P: Platform> FrameImages for ViewResources<P>
where
    FrameOf<P>: bobcat_core::FrameWriter,
{
    type PlatformFrame = FrameOf<P>;

    fn describe(
        &self,
        picture: &PictureKey,
        hint: ImageSizeHint,
    ) -> Option<bobcat_core::FrameImage<FrameOf<P>>> {
        match picture {
            PictureKey::Url(source) => images::describe(&self.resources, source, hint),
            PictureKey::Id(picture) if self.resources.media.borrow().contains(*picture) => {
                self.resources.media.borrow().describe(*picture)
            }
            PictureKey::Id(picture) => images::describe_picture(&self.resources, *picture, hint),
        }
    }

    fn take_frame(&self, picture: PictureId) -> Option<bobcat_core::TakenFrame<FrameOf<P>>> {
        let media = self.resources.media.borrow();
        if media.contains(picture) {
            return media.take_frame(picture);
        }
        drop(media);
        images::take_frame(&self.resources, picture)
    }

    fn want(&self, picture: &PictureKey, hint: ImageSizeHint) {
        match picture {
            PictureKey::Url(source) => images::want(&self.resources, source, hint),
            PictureKey::Id(picture) if self.resources.media.borrow().contains(*picture) => {
                self.resources.media.borrow().want(*picture);
            }
            PictureKey::Id(picture) => images::want_picture(&self.resources, *picture, hint),
        }
    }

    fn retain(&self, drawn: &[PictureKey]) {
        let ids = ids(drawn);
        self.resources.media.borrow_mut().retain(self.view, &ids);
        images::retain(&self.resources, self.view, &urls(drawn), &ids);
    }

    fn release(&self, pictures: &[PictureKey]) {
        images::release(&self.resources, &urls(pictures));
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
        assert_eq!(replacement.take_notes(), [] as [String; 0]);
        assert!(old.unregister(url));
        assert!(replacement.unregister(url));
    }
}
