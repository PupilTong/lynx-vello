//! The async pipeline: specifier → resolve → bytes → decode → cache.
//!
//! Sits directly on [`ResourceFetcher`], the host-injected protocol
//! `bobcat-core` owns. That crate is forbidden to decode images or own cache
//! policy, which is exactly the split this module completes from the other side:
//! the protocol moves bytes, this moves pixels.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::image::cache::{CacheKey, DecodeCache, HeaderCache, ResolvedKey, ResolvedMap};
use crate::image::capability::Capabilities;
use crate::image::decode::{DecodeRequest, DecodeResponse, Decoder, ImageHeader, PixelSize};
use crate::image::error::ImageError;
use crate::image::pixels::DecodedImage;
use crate::image::{data_url, decode_bytes, probe_bytes};
use crate::resource::{
    BufferedResourceRequest, CachePolicy, CacheTarget, ImageHints, PrefetchRequest, RequestContext,
    RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceDescriptor,
    ResourceFetcher, ResourceHints, ResourceKind, ResourceLocator, ResourcePriority,
    ResourceRequest,
};

/// Everything a loader is configured with.
///
/// The base URL lives here rather than behind a setter, because a loader is
/// shared through an `Arc` — `&mut self` is not reachable once anything has
/// spawned against it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct LoaderConfig {
    /// The owning template's bundle URL, handed to the fetcher as
    /// [`ResourceLocator::base_url`].
    ///
    /// **The fetcher performs the join**, not this module: that is what
    /// `resolve_locator` is for, and a host rewrite like Lynx's
    /// `res:///name → res:///id` is not a URL join at all, so the pre-join
    /// specifier has to reach the host intact.
    pub base_url: Option<Url>,
    /// The [`RequestId`] namespace this loader owns.
    ///
    /// The protocol requires a `RequestId` to be unique within one fetcher and
    /// delegates namespace allocation to the parties sharing it, so this has no
    /// safe default: two loaders over one fetcher that both defaulted to `0`
    /// would each issue `{namespace: 0, sequence: 0}` as their first request and
    /// break that uniqueness. [`LoaderConfig::new`] therefore takes it, and the
    /// view or engine that owns the fetcher is what assigns it.
    pub request_namespace: u64,
    /// Applies to every transport, not just the buffered one — a hostile or
    /// broken host must not be able to OOM the process through the stream or
    /// path branch that the buffered branch was hardened against.
    pub max_encoded_bytes: u64,
    pub decode_cache_bytes: u64,
    pub header_cache_entries: usize,
    /// In-flight decode cap. Bounded because a `spawn_blocking` decode cannot be
    /// aborted, so unbounded fan-out would let cancelled work starve the pool.
    pub max_concurrent_decodes: usize,
    /// In-flight *load* cap, acquired before the fetch and held until the decode
    /// ends.
    ///
    /// The decode permit alone does not bound memory: it is taken *after* the
    /// bytes are already in hand, so a hundred concurrent loads could each hold
    /// up to `max_encoded_bytes` while queueing for one of four decode slots.
    /// This is the bound that makes worst-case encoded residency
    /// `max_concurrent_loads * max_encoded_bytes` instead of unbounded.
    pub max_concurrent_loads: usize,
    pub decode: DecodeRequest,
    pub device_scale: Option<f32>,
    pub priority: ResourcePriority,
}

impl LoaderConfig {
    /// A configuration for the loader owning `request_namespace`.
    ///
    /// There is no `Default`: the namespace is not defaultable (see the field's
    /// own documentation), so it has to be supplied here.
    #[must_use]
    pub fn new(request_namespace: u64) -> Self {
        Self {
            request_namespace,
            ..Self::with_namespace_zero()
        }
    }

    /// Sets the encoded-byte ceiling every transport is held to.
    ///
    /// Builders rather than struct-update syntax: this type is
    /// `#[non_exhaustive]`, so `..LoaderConfig::new(0)` is a hard error outside
    /// this crate.
    #[must_use]
    pub const fn with_max_encoded_bytes(mut self, max_encoded_bytes: u64) -> Self {
        self.max_encoded_bytes = max_encoded_bytes;
        self
    }

    #[must_use]
    pub const fn with_decode_cache_bytes(mut self, decode_cache_bytes: u64) -> Self {
        self.decode_cache_bytes = decode_cache_bytes;
        self
    }

    #[must_use]
    pub const fn with_header_cache_entries(mut self, header_cache_entries: usize) -> Self {
        self.header_cache_entries = header_cache_entries;
        self
    }

    #[must_use]
    pub const fn with_max_concurrent_decodes(mut self, max_concurrent_decodes: usize) -> Self {
        self.max_concurrent_decodes = max_concurrent_decodes;
        self
    }

    #[must_use]
    pub const fn with_max_concurrent_loads(mut self, max_concurrent_loads: usize) -> Self {
        self.max_concurrent_loads = max_concurrent_loads;
        self
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: Option<Url>) -> Self {
        self.base_url = base_url;
        self
    }

    #[must_use]
    pub const fn with_decode(mut self, decode: DecodeRequest) -> Self {
        self.decode = decode;
        self
    }

    /// The single-loader configuration, namespace `0`.
    ///
    /// Correct only when exactly one loader shares the fetcher; anything else
    /// must use [`Self::new`].
    #[must_use]
    fn with_namespace_zero() -> Self {
        Self {
            base_url: None,
            request_namespace: 0,
            max_encoded_bytes: 64 << 20,
            decode_cache_bytes: 96 << 20,
            header_cache_entries: 512,
            max_concurrent_decodes: 4,
            max_concurrent_loads: 8,
            decode: DecodeRequest::default(),
            device_scale: None,
            priority: ResourcePriority::Normal,
        }
    }
}

/// Which cache a prefetch should warm.
///
/// `lynx.prefetchImage`'s two cache targets, split at the layer that owns each:
/// encoded bytes are the fetcher's, decoded pixels are ours.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum ImagePrefetchTarget {
    /// Delegated to [`ResourceFetcher::prefetch`].
    Encoded(CacheTarget),
    /// Decoded into the loader's own [`DecodeCache`].
    Decoded { target: Option<PixelSize> },
}

/// The loader's mutable state, behind **one** lock.
///
/// Three independent mutexes let a concurrent `load` and `clear_caches`
/// interleave into states no single operation can produce: pixels committed
/// with the header and mapping already cleared (an entry nothing can ever look
/// up again), or two racing loads combining one's header with the other's
/// pixels. Every commit and every clear touches all three, so they are one
/// critical section. It is held only across in-memory map updates — never
/// across a fetch, a decode, or any `.await`.
#[derive(Debug)]
struct LoaderState {
    decodes: DecodeCache,
    headers: HeaderCache,
    /// (specifier, target) → cache source, populated on every successful
    /// resolve. Without it, [`ImageLoader::cached`] — which must not await, so
    /// cannot resolve — would key on the pre-resolution specifier while
    /// [`ImageLoader::load`] keys on the resolved one, and every probe would
    /// miss.
    resolved: ResolvedMap,
}

/// Fetches, decodes and caches images for one view.
pub struct ImageLoader {
    fetcher: Arc<dyn ResourceFetcher>,
    /// The embedder-injected decoder. Shared because every blocking decode task
    /// carries its own handle to it.
    decoder: Arc<dyn Decoder>,
    state: Mutex<LoaderState>,
    /// `Arc` so a permit can be owned by the blocking decode task itself; see
    /// the acquisition site in `load`.
    decode_permits: Arc<Semaphore>,
    /// Taken before the fetch and released with the decode, so encoded buffers
    /// waiting for a decode slot are bounded too.
    load_permits: Arc<Semaphore>,
    sequence: AtomicU64,
    config: LoaderConfig,
    transport: ResourceCapability,
}

impl fmt::Debug for ImageLoader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `ResourceFetcher` has no `Debug` supertrait — it is a host-implemented
        // protocol object — so it is rendered by name, the same way
        // `bobcat-core` renders its own reader fields.
        formatter
            .debug_struct("ImageLoader")
            .field("fetcher", &"<dyn ResourceFetcher>")
            .field("decoder", &self.decoder)
            .field("transport", &self.transport)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ImageLoader {
    /// Builds a loader over `fetcher`, decoding through the injected `decoder`.
    ///
    /// There is no default decoder: the embedder chooses one — typically
    /// `image-decoders::platform_decoder()`, or its own [`Decoder`]
    /// implementation over an existing image pipeline — and the loader never
    /// second-guesses it.
    ///
    /// # Errors
    ///
    /// [`ImageError::NoTransport`] when the fetcher advertises none of
    /// [`ResourceCapability::BufferedResource`],
    /// [`ResourceCapability::ResourceStream`] or
    /// [`ResourceCapability::ResourcePath`]. Probed once here rather than per
    /// image, because only `resolve_locator` and `cancel_request` are mandatory
    /// in the protocol and a fetcher that can serve none of these can never
    /// serve an image.
    pub fn new(
        fetcher: Arc<dyn ResourceFetcher>,
        config: LoaderConfig,
        decoder: Arc<dyn Decoder>,
    ) -> Result<Self, ImageError> {
        let transport = [
            ResourceCapability::BufferedResource,
            ResourceCapability::ResourceStream,
            ResourceCapability::ResourcePath,
        ]
        .into_iter()
        .find(|capability| fetcher.supports_capability(*capability))
        .ok_or(ImageError::NoTransport)?;

        Ok(Self {
            fetcher,
            decoder,
            state: Mutex::new(LoaderState {
                decodes: DecodeCache::with_budget(config.decode_cache_bytes),
                headers: HeaderCache::with_capacity(config.header_cache_entries),
                resolved: ResolvedMap::with_capacity(config.header_cache_entries),
            }),
            decode_permits: Arc::new(Semaphore::new(config.max_concurrent_decodes.max(1))),
            load_permits: Arc::new(Semaphore::new(config.max_concurrent_loads.max(1))),
            sequence: AtomicU64::new(0),
            config,
            transport,
        })
    }

    /// The formats the injected decoder claims, and at which provenance tier.
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.decoder.capabilities()
    }

    /// Header-only load: resolve, then cache-or-fetch, then probe.
    ///
    /// This is what layout waits on. Probing is orders of magnitude cheaper than
    /// decoding, which is the whole reason the natural size and the pixels
    /// arrive on separate schedules.
    ///
    /// # Errors
    ///
    /// [`ImageError::Cancelled`], or any resolve/transport/probe failure.
    pub async fn header(
        &self,
        specifier: &str,
        cancel: CancellationToken,
    ) -> Result<ImageHeader, ImageError> {
        let resolved = self.resolve(specifier, None, cancel.clone()).await?;
        let source = cache_source(&resolved);

        if let Some(header) = self.lock().headers.get(&source) {
            return Ok(header);
        }
        let Some(_permit) = self.acquire(&self.load_permits, &cancel).await? else {
            return Err(ImageError::Cancelled);
        };
        let bytes = self.read_bytes(&resolved, cancel.clone()).await?;
        let header = probe_bytes(self.decoder.as_ref(), &bytes)?;
        if cancel.is_cancelled() {
            return Err(ImageError::Cancelled);
        }
        self.lock().headers.insert(Arc::clone(&source), header);
        Ok(header)
    }

    /// Full load. `target` is the used content box in device px, or `None` to
    /// decode at natural size.
    ///
    /// # Errors
    ///
    /// [`ImageError::Cancelled`], or any resolve/transport/decode failure.
    pub async fn load(
        &self,
        specifier: &str,
        target: Option<PixelSize>,
        cancel: CancellationToken,
    ) -> Result<DecodeResponse, ImageError> {
        let resolved = self.resolve(specifier, target, cancel.clone()).await?;
        let source = cache_source(&resolved);
        let key = CacheKey::new(Arc::clone(&source), target);

        // Consult the cache before the transport, not after. Resolving first is
        // still required — the key is the *resolved* source, so two specifiers a
        // host rewrites onto one resource share one entry — but everything past
        // this point is what the cache exists to avoid.
        if let Some(hit) = self.lock().decodes.get(&key) {
            return Ok(hit);
        }

        // Taken *before* the fetch, so the encoded buffer this load is about to
        // hold counts against a bound. Held until the decode ends.
        let Some(load_permit) = self.acquire(&self.load_permits, &cancel).await? else {
            return Err(ImageError::Cancelled);
        };

        let bytes = self.read_bytes(&resolved, cancel.clone()).await?;

        let decoder = Arc::clone(&self.decoder);
        let request = DecodeRequest {
            target_size: target,
            ..self.config.decode
        };
        // Waiting for a permit is itself cancellable: a queue of images whose
        // nodes are being recycled faster than they decode should shrink, not
        // sit blocked on a semaphore it no longer needs.
        let Some(permit) = self.acquire(&self.decode_permits, &cancel).await? else {
            return Err(ImageError::Cancelled);
        };

        // The permit moves *into* the blocking closure, so it is released when
        // the decode actually ends rather than when this future stops waiting
        // for it. `spawn_blocking` work is uncancellable once started, so
        // dropping the handle below leaves the decode running; releasing the
        // permit there instead would let a stream of cancelled loads run
        // unboundedly many concurrent decodes and starve tokio's blocking pool
        // — precisely under the churn the bound exists for.
        let decode = tokio::task::spawn_blocking(move || {
            let outcome = decode_bytes(decoder.as_ref(), &bytes, &request);
            drop(permit);
            drop(load_permit);
            outcome
        });

        // Returning promptly and discarding the result is the honest behaviour:
        // the decode drains against the bounded pool rather than being left to
        // publish pixels for a torn-down node.
        let Some(joined) = cancel.run_until_cancelled(decode).await else {
            return Err(ImageError::Cancelled);
        };
        let response = joined.map_err(|error| ImageError::decode_join(&error))??;

        // Re-checked here, not only before the decode: cancellation can land
        // while the blocking task is finishing, and publishing pixels for a node
        // that has since been torn down is the exact outcome the token was asked
        // to prevent.
        if cancel.is_cancelled() {
            return Err(ImageError::Cancelled);
        }

        // One critical section for the whole commit, so no observer can see the
        // pixels without the header that describes them.
        {
            let mut state = self.lock();
            state.headers.insert(Arc::clone(&source), response.header);
            state.decodes.insert(key, response.clone());
        }
        Ok(response)
    }

    /// Non-blocking decode-cache probe, for a caller already inside a frame
    /// commit that must not await.
    ///
    /// Misses until `specifier` has been resolved once by [`Self::load`] or
    /// [`Self::header`], because the cache is keyed on the *resolved* source and
    /// resolving is asynchronous.
    #[must_use]
    pub fn cached(&self, specifier: &str, target: Option<PixelSize>) -> Option<DecodedImage> {
        let mut state = self.lock();
        let source = state.resolved.get(&ResolvedKey::new(specifier, target))?;
        state
            .decodes
            .get(&CacheKey::new(source, target))
            .map(|response| response.image)
    }

    /// Non-blocking natural-size probe. Same resolution caveat as
    /// [`Self::cached`].
    ///
    /// A second mount of a known URL can publish its natural size in the same
    /// commit that creates the node, so the first frame lays out final.
    #[must_use]
    pub fn cached_header(&self, specifier: &str) -> Option<ImageHeader> {
        let mut state = self.lock();
        let source = state.resolved.get(&ResolvedKey::new(specifier, None))?;
        state.headers.get(&source)
    }

    /// Warms a cache ahead of render.
    ///
    /// # Errors
    ///
    /// As [`Self::load`], or [`ImageError::Resource`] from the fetcher's own
    /// prefetch.
    pub async fn prefetch(
        &self,
        specifier: &str,
        target: ImagePrefetchTarget,
    ) -> Result<(), ImageError> {
        let cancel = CancellationToken::new();
        match target {
            ImagePrefetchTarget::Decoded { target } => {
                self.load(specifier, target, cancel).await.map(|_| ())
            }
            ImagePrefetchTarget::Encoded(cache_target) => {
                let resolved = self.resolve(specifier, None, cancel.clone()).await?;
                if data_url::is_data_url(&resolved.url) {
                    // Nothing to warm: the bytes are the URL.
                    return Ok(());
                }
                self.fetcher
                    .prefetch(PrefetchRequest {
                        request: self.resource_request(&resolved, cancel),
                        target: cache_target,
                        max_bytes: self.config.max_encoded_bytes,
                    })
                    .await?;
                Ok(())
            }
        }
    }

    /// Drops every cached pixel, header and resolution, atomically.
    pub fn clear_caches(&self) {
        let mut state = self.lock();
        state.decodes.clear();
        state.headers.clear();
        state.resolved.clear();
    }

    // ------------------------------------------------------------- internals

    fn lock(&self) -> std::sync::MutexGuard<'_, LoaderState> {
        self.state.lock().expect("loader state")
    }

    /// Acquires a permit, giving up promptly if the caller cancels first.
    ///
    /// `Ok(None)` means cancelled; `Err` means the semaphore closed.
    async fn acquire(
        &self,
        semaphore: &Arc<Semaphore>,
        cancel: &CancellationToken,
    ) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, ImageError> {
        match cancel
            .run_until_cancelled(Arc::clone(semaphore).acquire_owned())
            .await
        {
            Some(Ok(permit)) => Ok(Some(permit)),
            Some(Err(_)) => Err(ImageError::Cancelled),
            None => Ok(None),
        }
    }

    fn next_context(&self, cancel: CancellationToken) -> RequestContext {
        RequestContext {
            id: RequestId {
                namespace: self.config.request_namespace,
                sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            },
            cancellation: cancel,
            priority: self.config.priority,
        }
    }

    /// Always runs, even for `data:` — a host's rewrite hook is entitled to turn
    /// one specifier into another, and only the fetcher knows that.
    async fn resolve(
        &self,
        specifier: &str,
        target: Option<PixelSize>,
        cancel: CancellationToken,
    ) -> Result<ResolvedLocator, ImageError> {
        let request = ResolveRequest {
            context: self.next_context(cancel.clone()),
            resource: ResourceDescriptor {
                locator: ResourceLocator {
                    specifier: Arc::from(specifier),
                    base_url: self.config.base_url.clone(),
                },
                kind: ResourceKind::Image,
                hints: ResourceHints::Image(ImageHints {
                    target_size_px: target,
                    device_scale: self.config.device_scale,
                    // v1 is static-only; a variant-serving CDN should see that.
                    allow_animation: false,
                }),
            },
            percent_decode: true,
        };
        // The host's resolver is arbitrary embedder code that may block on a
        // network round trip or a lock. Awaiting it bare made the whole load
        // uncancellable up to that point: a cancelled token could not unstick a
        // hung `resolve_locator`, so `load` would sit past any caller timeout.
        let Some(resolved) = cancel
            .run_until_cancelled(self.fetcher.resolve_locator(request))
            .await
        else {
            return Err(ImageError::Cancelled);
        };
        let resolved = resolved?;
        self.lock()
            .resolved
            .insert(ResolvedKey::new(specifier, target), cache_source(&resolved));
        Ok(resolved)
    }

    fn resource_request(
        &self,
        resolved: &ResolvedLocator,
        cancel: CancellationToken,
    ) -> ResourceRequest {
        ResourceRequest {
            context: self.next_context(cancel),
            resource: resolved.clone(),
            headers: http::HeaderMap::new(),
            cache_policy: CachePolicy::Default,
        }
    }

    /// The transport ladder, plus the `data:` short-circuit.
    async fn read_bytes(
        &self,
        resolved: &ResolvedLocator,
        cancel: CancellationToken,
    ) -> Result<Bytes, ImageError> {
        if cancel.is_cancelled() {
            return Err(ImageError::Cancelled);
        }
        let limit = self.config.max_encoded_bytes;
        if data_url::is_data_url(&resolved.url) {
            // Held to the same ceiling as every transport branch — the payload
            // is attacker-supplied on this path just as much as on the others.
            return data_url::decode(&resolved.url, limit).map(Bytes::from);
        }

        let request = self.resource_request(resolved, cancel.clone());
        let read = async {
            match self.transport {
                ResourceCapability::BufferedResource => {
                    let response = self
                        .fetcher
                        .fetch_resource(BufferedResourceRequest {
                            request,
                            max_bytes: limit,
                        })
                        .await?;
                    // `max_bytes` is a request to the host, not a guarantee from
                    // it: a buggy or hostile fetcher can return more, and
                    // trusting it made the budget advisory on this branch.
                    // `Bytes` is handed straight through — the previous
                    // `to_vec` copied up to the whole budget for nothing.
                    if response.bytes.len() as u64 > limit {
                        return Err(ImageError::EncodedTooLarge { limit });
                    }
                    Ok(response.bytes)
                }
                ResourceCapability::ResourceStream => {
                    let stream = self.fetcher.open_resource(request).await?;
                    let mut bytes = Vec::new();
                    // Read one byte PAST the ceiling so an overrun is *detected*
                    // rather than silently truncated. `take(limit)` cuts the
                    // stream at the limit, which for an oversized body whose
                    // prefix happens to be a complete image decodes and loads
                    // successfully — the budget would be enforced only by
                    // accident of framing.
                    let overrun = limit.saturating_add(1);
                    stream
                        .reader
                        .take(overrun)
                        .read_to_end(&mut bytes)
                        .await
                        .map_err(|error| {
                            ImageError::transport("reading resource stream", &error)
                        })?;
                    if bytes.len() as u64 > limit {
                        return Err(ImageError::EncodedTooLarge { limit });
                    }
                    Ok(Bytes::from(bytes))
                }
                ResourceCapability::ResourcePath => {
                    let resource = self.fetcher.fetch_resource_path(request).await?;
                    // tokio is built here without the `fs` feature, and a file
                    // read is blocking work regardless.
                    tokio::task::spawn_blocking(move || read_capped(&resource.path, limit))
                        .await
                        .map_err(|error| ImageError::decode_join(&error))?
                        .map(Bytes::from)
                }
                other => unreachable!("constructor rejected unusable transports, got {other:?}"),
            }
        };

        cancel
            .run_until_cancelled(read)
            .await
            .unwrap_or(Err(ImageError::Cancelled))
    }
}

/// The host's own cache key when it supplied one, else the resolved URL.
///
/// Never the pre-resolution specifier: two specifiers can resolve to one
/// resource, and a host's rewrite hook exists precisely to make them.
fn cache_source(resolved: &ResolvedLocator) -> Arc<str> {
    resolved
        .cache_key
        .clone()
        .unwrap_or_else(|| Arc::from(resolved.url.as_str()))
}

fn read_capped(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, ImageError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| ImageError::transport("stat resource path", &error))?;
    if metadata.len() > limit {
        return Err(ImageError::EncodedTooLarge { limit });
    }
    let bytes =
        std::fs::read(path).map_err(|error| ImageError::transport("read resource path", &error))?;
    // The file can grow between the stat and the read.
    if bytes.len() as u64 > limit {
        return Err(ImageError::EncodedTooLarge { limit });
    }
    Ok(bytes)
}
