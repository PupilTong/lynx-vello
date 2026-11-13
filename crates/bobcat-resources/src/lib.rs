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
//!   the main thread's `Image` element in the browser, each asked to downsample during decode
//!   ([`decode`]).
//! - **SVG documents parsed for the engine.** An SVG is not decoded to pixels: on every target, the
//!   browser included, it is parsed where a bitmap would be decoded, under a decode permit, with
//!   the engine's own parser ([`bobcat_core::ImageEvent::parse_document`]), and reported through
//!   [`ImageReports::parsed_document`]; the engine draws the document itself. Markup a page wrote
//!   (`<svg content>`) arrives as a document request with its bytes and is parsed the same way. A
//!   document holds no bitmap, so the memory tier and draw-sized decoding do not apply, and it is
//!   never retained: every request is parsed and reported, and the engine owns what it encodes.
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

use bobcat_core::peniko::ImageData;
use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{FrameImages, ImageReports, ImageResidency, ImageSizeHint, PreparsedStyleSheet};
use bytes::Bytes;
use rustc_hash::FxHashSet;
use url::Url;

pub mod cache;
pub mod decode;
mod executor;
#[cfg(not(target_arch = "wasm32"))]
pub mod file_body;
mod images;
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

/// What a [`ContainerInstaller`] registers through: the registry alone,
/// reachable from the thread the load finished on
/// ([`vant::registry::Registrar`], over this system's stylesheet type).
pub type Registrar = vant::registry::Registrar<PreparsedStyleSheet>;

/// The registered contents of one scope.
pub(crate) type Registry = vant::registry::Registry<PreparsedStyleSheet>;

/// The platform [`Resources::new`] builds, and the one [`Resources`] and
/// [`ViewResources`] name when no other is: [`NativePlatform`]. Native
/// only: the browser's platform is the embedder's (`bobcat-wasm`).
#[cfg(not(target_arch = "wasm32"))]
pub type DefaultPlatform = platform::NativePlatform;

/// The directory inside [`DiskCacheConfig::dir`] the code cache keeps its
/// entries in: a Store of its own, which the byte tier's scan leaves alone.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const CODE_CACHE_DIR: &str = "code";

/// The disk cache's directory and budgets, natively: what
/// [`NativePlatform::new`] opens an [`FsStore`] over for the byte tier, and
/// another over `dir/code` for the code cache.
#[derive(Clone, Debug)]
pub struct DiskCacheConfig {
    pub dir: std::path::PathBuf,
    /// The byte tier's budget.
    pub budget_bytes: u64,
    /// The code cache's budget, in stored entry bytes.
    pub code_budget_bytes: u64,
}

impl DiskCacheConfig {
    /// The platform's per-user cache directory for Bobcat, or `None` when
    /// the environment does not name one. The code cache gets its default
    /// budget, [`ArtifactConfig::DEFAULT_BUDGET_BYTES`] (64 MiB).
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

/// Everything a [`Resources`] is built with, beside its [`Platform`].
/// `Default` is a working configuration for any host.
#[derive(Clone, Debug)]
pub struct ResourcesConfig {
    /// What relative stylesheet, fetch and image URLs resolve against, by
    /// URL rules — the page's own URL. A font URL arrives absolute, as the
    /// document resolved it, and so does every script URL: an entry or a
    /// synchronous load resolved against the view's
    /// [`ViewSources::base_url`](bobcat_core::ViewSources::base_url), a
    /// worker script against `__Card__` (the view's card URL, or else the
    /// entry's response URL) and an import against its importer's. This must
    /// be that same view base: a lazy container is
    /// fetched, and its sections registered, by this one, and each section is
    /// loaded synchronously by the view's.
    pub base_url: Option<Url>,
    /// The memory tier's budget for decoded bitmaps. Best-effort: the frame
    /// being drawn is never evicted, and a single bitmap larger than the
    /// budget still has to exist while it is on screen.
    pub memory_budget_bytes: usize,
    /// The largest per-axis size of an image's first decode, before any
    /// frame has said how large it draws it. Also bounded by what the rasterizer can
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
    /// What a plain
    /// [`SourceRequest::Fetch`](bobcat_core::resource::SourceRequest::Fetch)
    /// offers its bytes to before answering: the hook that makes a fetched
    /// Lynx container's sections loadable afterwards.
    ///
    /// `None`, the default, changes nothing about the fetch — it still
    /// completes, exactly as an image fetch does. A host that wants
    /// `lynx.fetchBundle` to install lazy bundles passes
    /// `bobcat_source::BundleInstaller`, which registers a fetched container
    /// by the same `ContainerSources` path a page's own takes.
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

/// What a [`ResourceFetcher::fetch_probe`] reads, and the base the relative
/// stylesheets, fetches and images this system is asked for resolve against.
/// An entry or a synchronous load, which is how a lazy container's section
/// is loaded, arrives resolved against the view's base, which is why the two
/// must be equal.
///
/// Its own `Arc` rather than a field of [`Shared`] for one reason: the probe
/// is called from `bobcat-main` and `bobcat-workers` while the fetcher itself
/// never leaves the embedder's thread, so what crosses has to be `Send +
/// Sync` on every target — and on wasm32 a `SharedHandle` is an `Rc`. Two
/// mutexes and a set of URLs are.
#[derive(Debug, Default)]
pub(crate) struct FetchIndex {
    base_url: Mutex<Option<Url>>,
    /// Every URL a [`SourceKind::Fetch`](sources::SourceKind) load completed
    /// successfully for. Written after the container installer ran, so a
    /// fetch whose install failed is not in it; a failed fetch is never
    /// remembered either, as native remembers no failure.
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

    /// Remembers one completed fetch, by its **resolved request URL** — the
    /// same URL `sources::request` resolved the specifier to.
    pub(crate) fn remember(&self, url: &Url) {
        self.fetched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(url.clone());
    }

    /// Whether a fetch of `specifier` has already completed, resolving it the
    /// way a request would: joined to the current base by URL rules, or
    /// parsed on its own when there is no base. A specifier that resolves to
    /// nothing was never fetched, so it is simply `false`.
    ///
    /// The base is read here rather than captured once, because
    /// [`Resources::set_base_url`] can move it.
    fn contains(&self, specifier: &str) -> bool {
        let Some(url) = self.resolve(specifier) else {
            return false;
        };
        self.fetched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&url)
    }

    /// `specifier` resolved the way a request resolves it — joined to the
    /// current base by URL rules, or parsed on its own when there is none —
    /// or `None` when it resolves to nothing.
    fn resolve(&self, specifier: &str) -> Option<Url> {
        transport::join(specifier, self.base_url().as_ref()).ok()
    }
}

/// The handle jobs hold on [`Shared`] and the platform parts in it: atomic
/// natively, where a job's task and its blocking closures share it across
/// threads, and plain in the browser, where nothing does and the decoder it
/// holds is thread-bound anyway.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Handle<T> = Arc<T>;
#[cfg(target_arch = "wasm32")]
pub(crate) type Handle<T> = Rc<T>;

/// The handle jobs hold on [`Shared`].
pub(crate) type SharedHandle<P> = Handle<Shared<P>>;

/// The code cache's persistent half: the driver over the platform's
/// artifact store, shared by every scope of one system.
///
/// The engine fills the process-wide memory tier ([`Artifacts::global`]);
/// this preloads a script's stored bytecode into it before the script is
/// delivered and writes what the engine compiled back
/// ([`flush_code_cache`]).
///
/// - Over a synchronous [`Store`] ([`ArtifactCache`]): the preload runs in the source load's CPU
///   half ([`Self::preload`]), a flush runs on the blocking pool natively and inline in the
///   browser, and when the last scope lets go, whatever is still queued is written once more,
///   synchronously.
/// - Over an [`AsyncStore`] ([`AsyncArtifactCache`]): the store is opened by a task started with
///   the system, and the first call that needs it awaits that open. The preload is awaited after a
///   module's text is prepared and before its completion is delivered ([`Self::preload_async`]),
///   and a flush is a task of its own. There is no flush when the last scope lets go: what the
///   engine compiled after the last serviced turn is compiled again next time.
pub(crate) struct CodeCache<S: Store, A: AsyncStore> {
    memory: Arc<Artifacts>,
    driver: CodeDriver<S, A>,
    /// Whether a flush is queued or running, so a turn starts at most one.
    flushing: std::sync::atomic::AtomicBool,
}

enum CodeDriver<S: Store, A: AsyncStore> {
    Blocking(ArtifactCache<S>),
    Async {
        /// The store, until the open takes it.
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

    /// The asynchronous driver, opened by the first call that asks. `None`
    /// over a synchronous store, or when an open was dropped before it
    /// finished (the system shutting down), which leaves no store to open.
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

    /// Over a synchronous store, admits every stored artifact of `text` to
    /// the memory tier, for a script about to be delivered as exactly
    /// `text`. A text the engine would not look up
    /// ([`bobcat_core::MIN_CACHED_SOURCE_BYTES`]) is not hashed. Over an
    /// asynchronous store this does nothing: [`Self::preload_async`] does it.
    pub(crate) fn preload(&self, text: &str) {
        if let CodeDriver::Blocking(cache) = &self.driver
            && text.len() >= bobcat_core::MIN_CACHED_SOURCE_BYTES
        {
            cache.preload(SourceDigest::of(text.as_bytes()), transport::now());
        }
    }

    /// [`Self::preload`] over an asynchronous store, awaited before the
    /// script's completion is delivered. Over a synchronous store this does
    /// nothing: the preload already ran.
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

/// Clears [`CodeCache::flushing`] when an asynchronous flush ends, or when
/// its task is dropped first.
struct Flushing<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for Flushing<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

impl<S: Store, A: AsyncStore> Drop for CodeCache<S, A> {
    /// Over a synchronous store, writes what is still queued. Over an
    /// asynchronous one nothing is written: there is no task left to await
    /// the store.
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

/// The part of the system every job may touch. It holds no piece of the
/// [`executor::Executor`], so no job can keep the fetcher's runtime alive.
pub(crate) struct Shared<P: Platform> {
    transports: Transports<P>,
    /// The code cache's Store side, when the platform handed over an
    /// artifact Store; shared by every scope, like the byte tier.
    code: Option<Handle<CodeCache<P::Store, P::AsyncStore>>>,
    /// The base every specifier resolves against, and the URLs a plain fetch
    /// has completed for. Behind an `Arc` of its own because
    /// [`ResourceFetcher::fetch_probe`] hands a reader of it to the engine
    /// threads, while everything else in [`Shared`] stays where it is.
    fetches: Arc<FetchIndex>,
    initial_decode_bound: u32,
    downsample_ratio: f32,
    decoder: Handle<P::Decoder>,
    completions: tokio::sync::mpsc::UnboundedSender<Completion>,
    wakeup: Wakeup,
    log_to_stderr: bool,
    container_installer: Option<Arc<dyn ContainerInstaller<PreparsedStyleSheet>>>,
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

impl<P: Platform> Shared<P> {
    /// Offers one fetch's bytes to the container installer, if this host has
    /// one, and says whether anything was registered.
    ///
    /// No installer is not a failure: a plain fetch with nothing to make of
    /// its bytes is still a fetch that completed.
    ///
    /// The registrar is built here rather than handed in, because what it
    /// writes through is this very [`Shared`]'s registry.
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

    /// Keeps `message` for [`Resources::take_notes`], logging it if this
    /// system logs.
    fn note(&self, message: String) {
        if self.log_to_stderr {
            eprintln!("bobcat-resources: {message}");
        }
        self.notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

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
    /// with `header`, at most `max` large, with the platform's decoder,
    /// blocking the calling thread.
    fn decode_bytes(
        &self,
        bytes: &[u8],
        format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        vant::ImageDecoder::decode_blocking(&*self.decoder, bytes, format, header, max)
    }

    /// [`Self::decode_bytes`] without blocking, for the browser's loads.
    #[cfg(target_arch = "wasm32")]
    async fn decode_bytes_async(
        &self,
        bytes: Bytes,
        format: mime::ImageFormat,
        header: Option<image_header::ImageHeader>,
        max: (u32, u32),
    ) -> Result<decode::Bitmap, decode::DecodeError> {
        vant::ImageDecoder::decode(&*self.decoder, bytes, format, header, max).await
    }
}

/// The shared resource system: registry, caches, job executor and decoder.
///
/// Cheap to clone — every clone is the same system — and bound to the
/// thread that built it, which must be the thread that builds the views it
/// serves (the painter's). It is also the only holder of the native
/// executor: when the last clone of the last scope goes, the fetcher's
/// runtime is shut down, on this thread.
///
/// The platform parameter defaults to [`DefaultPlatform`] natively; on
/// wasm32 there is none, and the embedder names its own.
pub struct Resources<
    #[cfg(not(target_arch = "wasm32"))] P: Platform = DefaultPlatform,
    #[cfg(target_arch = "wasm32")] P: Platform,
> {
    shared: SharedHandle<P>,
    /// Shared by every scope, held by nothing a job can reach.
    #[cfg(not(target_arch = "wasm32"))]
    executor: Arc<executor::Executor>,
    local: Rc<RefCell<ImageState>>,
    style_cache: Rc<RefCell<sources::StyleCache>>,
}

impl<P: Platform> Clone for Resources<P> {
    fn clone(&self) -> Self {
        Self {
            shared: SharedHandle::clone(&self.shared),
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::clone(&self.executor),
            local: Rc::clone(&self.local),
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
    /// Builds the system over [`DefaultPlatform`] with no persistent store.
    /// What tests and `bobcat-server` build; [`Resources::with_platform`]
    /// says the rest.
    ///
    /// # Panics
    ///
    /// As [`Resources::with_platform`].
    #[must_use]
    pub fn new(config: ResourcesConfig, wakeup: impl Fn() + Send + Sync + 'static) -> Self {
        let (platform, notes) = DefaultPlatform::new(None);
        Self::with_platform(config, platform, notes, wakeup)
    }
}

impl<P: Platform> Resources<P> {
    /// Builds the system over `platform`. `notes` are what building the
    /// platform left out (the second half of [`NativePlatform::new`]'s
    /// answer); they open [`Resources::take_notes`]. The byte tier is opened
    /// over the platform's byte Store, under that Store's budget, unless its
    /// transport applies HTTP cache semantics itself; the code cache over
    /// its artifact Store, whatever the transport.
    ///
    /// `wakeup` is called whenever a load completes between painter turns —
    /// natively from the executor's driver thread, in the browser from the
    /// Render Worker's own task, there being no driver there. Pass the same
    /// wakeup the view was given, so the completion is answered by a turn.
    ///
    /// Natively that wakeup must not block, and must not take a lock the
    /// thread dropping the last [`Resources`] can be holding: the drop joins
    /// the driver, so it waits for a wakeup already running.
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
    pub fn with_platform(
        config: ResourcesConfig,
        platform: P,
        notes: Vec<String>,
        wakeup: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let Parts {
            fetch,
            decoder,
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
            completions,
            wakeup: Arc::new(wakeup),
            log_to_stderr: config.log_to_stderr,
            container_installer: config.container_installer.clone(),
            notes: Mutex::new(notes),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            fetch_hook: Mutex::new(None),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            decode_hook: Mutex::new(None),
        });
        let resources = Self {
            shared,
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::new(executor::Executor::new(
                config.worker_threads,
                config.decode_parallelism,
            )),
            style_cache: Rc::default(),
            local: Rc::new(RefCell::new(ImageState::new(
                ImageResidency::new(
                    config.memory_budget_bytes,
                    initial_decode_bound,
                    downsample_ratio,
                ),
                receiver,
            ))),
        };
        // An asynchronous code store is opened by a task of its own, so its
        // scan runs while the first page's sources are fetched.
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

    /// Runs `task` beside the system's loads: natively on the executor, in
    /// the browser as a local task of the Render Worker.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.executor.spawn(task);
    }

    /// Runs `task` beside the system's loads: natively on the executor, in
    /// the browser as a local task of the Render Worker.
    #[cfg(target_arch = "wasm32")]
    #[expect(clippy::unused_self, reason = "the native twin spawns on the executor")]
    fn spawn(&self, task: impl Future<Output = ()> + 'static) {
        wasm_bindgen_futures::spawn_local(task);
    }

    /// An independent page resource scope sharing only the executor, the
    /// platform decoder, the transport and the byte tier. Registrations, relative URL base,
    /// decoded images, stylesheet responses and completion queues start empty. Late image results
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
                transports: self.shared.transports.for_new_scope(),
                code: self.shared.code.clone(),
                // A fresh index, like the fresh registry beside it: a new
                // scope has fetched nothing and has no base yet.
                fetches: Arc::new(FetchIndex::default()),
                initial_decode_bound: self.shared.initial_decode_bound,
                downsample_ratio: self.shared.downsample_ratio,
                decoder: Handle::clone(&self.shared.decoder),
                completions,
                wakeup: self.shared.wakeup.clone(),
                log_to_stderr: self.shared.log_to_stderr,
                container_installer: self.shared.container_installer.clone(),
                notes: Mutex::new(Vec::new()),
                #[cfg(all(test, not(target_arch = "wasm32")))]
                fetch_hook: Mutex::new(None),
                #[cfg(all(test, not(target_arch = "wasm32")))]
                decode_hook: Mutex::new(None),
            }),
            #[cfg(not(target_arch = "wasm32"))]
            executor: Arc::clone(&self.executor),
            style_cache: Rc::default(),
            local: Rc::new(RefCell::new(ImageState::new(
                ImageResidency::new(
                    self.local.borrow().residency.budget(),
                    self.shared.initial_decode_bound,
                    self.shared.downsample_ratio,
                ),
                receiver,
            ))),
        }
    }

    /// Registers `bytes` under `url`, replacing any earlier registration.
    /// `media_type` labels them the way a `Content-Type` would; without it
    /// they are sniffed. Returns the normalized URL they answer to. Invalidates
    /// any cached stylesheet response for that URL.
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

    /// Registers module bytecode under `url`, as
    /// [`Registrar::register_module_bytecode`] does. Invalidates any cached
    /// stylesheet response for that URL.
    ///
    /// # Errors
    ///
    /// [`RegisterError::InvalidUrl`] for a URL that does not parse.
    pub fn register_module_bytecode(
        &self,
        url: &str,
        bytes: impl Into<Bytes>,
    ) -> Result<Url, RegisterError> {
        let url = parse_registration_url(url)?;
        self.style_cache.borrow_mut().remove(&url);
        self.shared
            .transports
            .registry
            .insert(&url, Registered::ModuleBytecode(bytes.into()));
        Ok(url)
    }

    /// Registers a stylesheet the host already parsed. It answers a stylesheet
    /// source request pre-parsed, and no other request at all — it has no
    /// bytes to give one. Replaces any cached stylesheet response for that URL.
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

    /// Runs `install` off this thread with a [`Registrar`] onto this
    /// system, and holds every source request for a URL under `container`
    /// until it has run: such a request is then answered from what it
    /// registered, or fails with the message it failed with.
    ///
    /// This is how a view boots while its container is still being decoded:
    /// the embedder names the container's URLs to the view, creates it, and
    /// the view's first loads of them wait here rather than fail. A URL is
    /// under `container` when, its query and fragment aside, it is the
    /// container URL's path or below it — every URL one container registers
    /// is (`bobcat-source`'s container URL rule).
    ///
    /// Natively `install` runs on the blocking pool; in the browser, as a
    /// task of this thread, after the caller yields.
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
            // Released only once everything is registered: a request that no
            // longer finds the hold finds the registrations, and one that
            // found it waits for this answer.
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

    /// Forgets a registration and its cached stylesheet response.
    /// Images already decoded from it stay decoded.
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

    /// Forgets every registration and its cached stylesheet response.
    pub fn clear_registered(&self) {
        self.style_cache
            .borrow_mut()
            .retain(|url, _| !self.shared.transports.registry.contains(url));
        self.shared.transports.registry.clear();
    }

    /// What relative stylesheet, fetch and image URLs resolve against. It
    /// must equal the view's
    /// [`ViewSources::base_url`](bobcat_core::ViewSources::base_url), which
    /// an entry or a synchronous load arrives resolved against. Every other
    /// script URL arrives absolute as well, a worker script resolved against
    /// `__Card__` (the view's card URL, or else the entry's response URL) and
    /// an import against its importer's, and so
    /// does a font URL, as the document resolved it.
    #[must_use]
    pub fn base_url(&self) -> Option<Url> {
        self.shared.fetches.base_url()
    }

    /// Moves the base [`Self::base_url`] reads. Set it before
    /// [`LynxGroup::create_lynx_view`](bobcat_core::LynxGroup::create_lynx_view),
    /// to the view's own base: a base moved after that no longer equals the
    /// view's, and a lazy container's sections would then be registered
    /// under URLs the view's realms never load.
    pub fn set_base_url(&self, base_url: Option<Url>) {
        self.shared.fetches.set_base_url(base_url);
    }

    /// Bytes the memory tier holds: decoded bitmaps, plus the encoded
    /// images nothing else could restore. A parsed document (SVG) is never
    /// retained, so it counts nothing.
    #[must_use]
    pub fn memory_used_bytes(&self) -> usize {
        let state = self.local.borrow();
        state.residency.bitmap_bytes() + state.residency.encoded_bytes()
    }

    /// Changes the bitmap budget, evicting what no longer fits.
    pub fn set_memory_budget_bytes(&self, budget: usize) {
        self.local.borrow_mut().residency.set_budget(budget);
    }

    /// Whether `source` currently has decoded pixels in memory.
    #[must_use]
    pub fn is_resident(&self, source: &str) -> bool {
        self.local.borrow().residency.is_resident(source)
    }

    /// The resident bitmap's size for `source`, if it has one.
    #[must_use]
    pub fn resident_size(&self, source: &str) -> Option<(u32, u32)> {
        self.local.borrow().residency.resident_size(source)
    }

    /// Whether `source` has been asked for, through
    /// [`ResourceFetcher::request_image`]. A document request
    /// ([`ResourceFetcher::request_document`]) files nothing, so it never
    /// makes a source known.
    #[must_use]
    pub fn knows_image(&self, source: &str) -> bool {
        self.local.borrow().residency.knows(source)
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
    /// [`ImageReports`] and its own working set, which this scope pins
    /// beside every other view's until the value is dropped.
    #[must_use]
    pub fn for_view(&self, reports: ImageReports) -> ViewResources<P> {
        let view = self.local.borrow_mut().residency.new_view();
        ViewResources {
            resources: self.clone(),
            reports,
            view,
        }
    }

    /// The builder [`bobcat_core::LynxGroup::create_lynx_view`] takes.
    pub fn builder(&self) -> impl FnOnce(ImageReports) -> ViewResources<P> + 'static {
        let resources = self.clone();
        move |reports| resources.for_view(reports)
    }

    pub(crate) fn note(&self, message: String) {
        self.shared.note(message);
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

    fn register_module_bytecode(&self, url: &str, bytes: Bytes) -> Result<Url, RegisterError> {
        Self::register_module_bytecode(self, url, bytes)
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

/// Writes what the engine compiled since the last flush to the code cache's
/// store, when there is a store, something is queued and no flush is already
/// queued or running. Asked once per embedder turn, from
/// [`ResourceFetcher::service_images`]: when nothing is queued it costs one
/// atomic load.
///
/// Over a synchronous store the write runs on the blocking pool natively,
/// where a panic in it is a note, and here in the browser. Over an
/// asynchronous store it is a task that awaits the store.
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
    /// A flush over a synchronous store; nothing over an asynchronous one.
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

/// One view's end of the system: the [`ResourceFetcher`] it is built with.
pub struct ViewResources<
    #[cfg(not(target_arch = "wasm32"))] P: Platform = DefaultPlatform,
    #[cfg(target_arch = "wasm32")] P: Platform,
> {
    resources: Resources<P>,
    reports: ImageReports,
    /// This view's working set in the scope's image residency.
    view: ViewKey,
}

impl<P: Platform> Drop for ViewResources<P> {
    /// The view's working set stops counting: what only it pinned may be
    /// evicted.
    fn drop(&mut self) {
        // `try_borrow_mut`: a panic that unwinds through a borrow of the
        // image state must not turn into an abort here.
        if let Ok(mut state) = self.resources.local.try_borrow_mut() {
            state.residency.release_view(self.view);
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
    /// The shared system behind this view.
    #[must_use]
    pub fn resources(&self) -> &Resources<P> {
        &self.resources
    }
}

impl<P: Platform> ResourceFetcher for ViewResources<P> {
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

    fn service_images(&self) {
        images::service(&self.resources);
        flush_code_cache(&self.resources);
    }

    /// This view's answer to "already fetched?", read from either engine
    /// thread. It resolves the specifier the way a request would and looks it
    /// up in the set a completed [`SourceRequest::Fetch`] writes; nothing
    /// else about the load is reachable through it, and it starts nothing.
    fn fetch_probe(&self) -> Option<bobcat_core::resource::FetchProbe> {
        let index = Arc::clone(&self.resources.shared.fetches);
        Some(Arc::new(move |specifier: &str| index.contains(specifier)))
    }

    /// This view's answer to "is a stylesheet registered here?", read from
    /// `bobcat-main` by `__LoadStyleSheet`. It resolves the specifier the way
    /// a request would and looks it up in the registry, nothing else: an
    /// unregistered URL is `false` without any transport being consulted, so
    /// a stylesheet this system would have to fetch over the network is not
    /// one a container carries. Any registration counts, a pre-parsed sheet
    /// or CSS bytes; whether it parses is the adoption's business.
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

impl<P: Platform> FrameImages for ViewResources<P> {
    fn read(&self, source: &str, hint: ImageSizeHint) -> Option<ImageData> {
        images::read(&self.resources, source, hint)
    }

    fn retain(&self, frame: &[Arc<str>]) {
        images::retain(&self.resources, self.view, frame);
    }

    fn describe(&self, source: &str, hint: ImageSizeHint) -> Option<bobcat_core::FrameImage> {
        images::describe(&self.resources, source, hint)
    }

    fn release(&self, sources: &[Arc<str>]) {
        images::release(&self.resources, sources);
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
