//! The async pipeline: specifier → resolve → bytes → decode → cache.
//!
//! Sits directly on [`ResourceFetcher`], the host-injected protocol
//! `bobcat-engine` owns. That crate is forbidden to decode images or own cache
//! policy, which is exactly the split this module completes from the other side:
//! the protocol moves bytes, this moves pixels.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bobcat_engine::resource::{
    BufferedResourceRequest, CachePolicy, CacheTarget, ImageHints, PrefetchRequest, RequestContext,
    RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceDescriptor,
    ResourceFetcher, ResourceHints, ResourceKind, ResourceLocator, ResourcePriority,
    ResourceRequest,
};
use rustc_hash::FxHashMap;
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::cache::{CacheKey, DecodeCache, HeaderCache};
use crate::decode::{DecodeRequest, DecodeResponse, ImageHeader, PixelSize};
use crate::error::ImageError;
use crate::pixels::DecodedImage;
use crate::registry::{BackendRegistry, Capabilities};
use crate::{data_url, decode_bytes, probe_bytes};

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
    /// **The fetcher performs the join**, not this crate: that is what
    /// `resolve_locator` is for, and a host rewrite like Lynx's
    /// `res:///name → res:///id` is not a URL join at all, so the pre-join
    /// specifier has to reach the host intact.
    pub base_url: Option<Url>,
    /// The [`RequestId`] namespace this loader owns. The protocol delegates
    /// namespace allocation to the parties sharing a fetcher; the loader owns
    /// its own sequence within it.
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
    pub decode: DecodeRequest,
    pub device_scale: Option<f32>,
    pub priority: ResourcePriority,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            request_namespace: 0,
            max_encoded_bytes: 64 << 20,
            decode_cache_bytes: 96 << 20,
            header_cache_entries: 512,
            max_concurrent_decodes: 4,
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
    /// Decoded into this crate's own [`DecodeCache`].
    Decoded { target: Option<PixelSize> },
}

/// Fetches, decodes and caches images for one view.
pub struct ImageLoader {
    fetcher: Arc<dyn ResourceFetcher>,
    registry: BackendRegistry,
    decodes: Mutex<DecodeCache>,
    headers: Mutex<HeaderCache>,
    /// Specifier → cache source, populated on every successful resolve.
    ///
    /// Without this, [`Self::cached`] — which must not await, so cannot resolve
    /// — would key on the pre-resolution specifier while [`Self::load`] keys on
    /// the resolved one, and every probe would miss.
    resolved: Mutex<FxHashMap<Arc<str>, Arc<str>>>,
    permits: Semaphore,
    sequence: AtomicU64,
    config: LoaderConfig,
    transport: ResourceCapability,
}

impl fmt::Debug for ImageLoader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `ResourceFetcher` has no `Debug` supertrait — it is a host-implemented
        // protocol object — so it is rendered by name, the same way
        // `bobcat-engine` renders its own reader fields.
        formatter
            .debug_struct("ImageLoader")
            .field("fetcher", &"<dyn ResourceFetcher>")
            .field("registry", &self.registry)
            .field("transport", &self.transport)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ImageLoader {
    /// Builds a loader over `fetcher`, using the best backend set this machine
    /// offers.
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
    ) -> Result<Self, ImageError> {
        Self::with_registry(fetcher, config, BackendRegistry::detect())
    }

    /// [`Self::new`] with an explicit backend set, for tests that must not
    /// depend on what the host machine happens to provide.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_registry(
        fetcher: Arc<dyn ResourceFetcher>,
        config: LoaderConfig,
        registry: BackendRegistry,
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
            registry,
            decodes: Mutex::new(DecodeCache::with_budget(config.decode_cache_bytes)),
            headers: Mutex::new(HeaderCache::with_capacity(config.header_cache_entries)),
            resolved: Mutex::new(FxHashMap::default()),
            permits: Semaphore::new(config.max_concurrent_decodes.max(1)),
            sequence: AtomicU64::new(0),
            config,
            transport,
        })
    }

    /// The formats decodable on this machine, and at which provenance tier.
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.registry.capabilities()
    }

    #[must_use]
    pub const fn registry(&self) -> &BackendRegistry {
        &self.registry
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

        if let Some(header) = self.headers.lock().expect("header cache").get(&source) {
            return Ok(header);
        }
        let bytes = self.read_bytes(&resolved, cancel).await?;
        let header = probe_bytes(&self.registry, &bytes)?;
        self.headers
            .lock()
            .expect("header cache")
            .insert(Arc::clone(&source), header);
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
        if let Some(hit) = self.cache_hit(&key, &source) {
            return Ok(hit);
        }

        let bytes = self.read_bytes(&resolved, cancel.clone()).await?;

        let registry = self.registry.clone();
        let request = DecodeRequest {
            target_size: target,
            ..self.config.decode
        };
        let permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| ImageError::Cancelled)?;
        let decode = tokio::task::spawn_blocking(move || decode_bytes(&registry, &bytes, &request));

        // `spawn_blocking` work is genuinely uncancellable once started, so the
        // token cannot abort it. Returning promptly and discarding the result is
        // the honest behaviour: the decode drains against a bounded permit pool
        // rather than being left to publish pixels for a torn-down node.
        let Some(joined) = cancel.run_until_cancelled(decode).await else {
            return Err(ImageError::Cancelled);
        };
        drop(permit);
        let response = joined.map_err(|error| ImageError::decode_join(&error))??;
        if cancel.is_cancelled() {
            return Err(ImageError::Cancelled);
        }

        self.headers
            .lock()
            .expect("header cache")
            .insert(Arc::clone(&source), response.header);
        self.decodes
            .lock()
            .expect("decode cache")
            .insert(key, response.image.clone());
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
        let source = self.known_source(specifier)?;
        self.decodes
            .lock()
            .expect("decode cache")
            .get(&CacheKey::new(source, target))
    }

    /// Non-blocking natural-size probe. Same resolution caveat as
    /// [`Self::cached`].
    ///
    /// A second mount of a known URL can publish its natural size in the same
    /// commit that creates the node, so the first frame lays out final.
    #[must_use]
    pub fn cached_header(&self, specifier: &str) -> Option<ImageHeader> {
        let source = self.known_source(specifier)?;
        self.headers.lock().expect("header cache").get(&source)
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

    pub fn clear_caches(&self) {
        self.decodes.lock().expect("decode cache").clear();
        self.headers.lock().expect("header cache").clear();
        self.resolved.lock().expect("resolution map").clear();
    }

    // ------------------------------------------------------------- internals

    /// A complete cached answer, or `None`.
    ///
    /// Both halves are required: the pixels alone cannot rebuild a
    /// [`DecodeResponse`], because `header.natural_size` is the *source* size
    /// that `object-fit` resolves against and a downsampled entry no longer
    /// carries it. The header cache is much larger than the decode cache, so in
    /// practice the pixels are what expires first.
    fn cache_hit(&self, key: &CacheKey, source: &str) -> Option<DecodeResponse> {
        let image = self.decodes.lock().expect("decode cache").get(key)?;
        let header = self.headers.lock().expect("header cache").get(source)?;
        Some(DecodeResponse {
            image,
            header,
            acceleration: self.registry.effective_tier(header.format),
            backend: "cache",
        })
    }

    fn known_source(&self, specifier: &str) -> Option<Arc<str>> {
        self.resolved
            .lock()
            .expect("resolution map")
            .get(specifier)
            .cloned()
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
            context: self.next_context(cancel),
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
        let resolved = self.fetcher.resolve_locator(request).await?;
        self.resolved
            .lock()
            .expect("resolution map")
            .insert(Arc::from(specifier), cache_source(&resolved));
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
    ) -> Result<Vec<u8>, ImageError> {
        if cancel.is_cancelled() {
            return Err(ImageError::Cancelled);
        }
        if data_url::is_data_url(&resolved.url) {
            return data_url::decode(&resolved.url);
        }

        let limit = self.config.max_encoded_bytes;
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
                    Ok(response.bytes.to_vec())
                }
                ResourceCapability::ResourceStream => {
                    let stream = self.fetcher.open_resource(request).await?;
                    let mut bytes = Vec::new();
                    // The same ceiling the buffered branch gets: `take` stops at
                    // the limit rather than letting a host stream forever.
                    stream
                        .reader
                        .take(limit)
                        .read_to_end(&mut bytes)
                        .await
                        .map_err(|error| {
                            ImageError::transport("reading resource stream", &error)
                        })?;
                    Ok(bytes)
                }
                ResourceCapability::ResourcePath => {
                    let resource = self.fetcher.fetch_resource_path(request).await?;
                    // tokio is built here without the `fs` feature, and a file
                    // read is blocking work regardless.
                    tokio::task::spawn_blocking(move || read_capped(&resource.path, limit))
                        .await
                        .map_err(|error| ImageError::decode_join(&error))?
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
        return Err(ImageError::transport(
            "resource path",
            &std::io::Error::other(format!(
                "{} bytes exceeds the {limit}-byte encoded-image budget",
                metadata.len()
            )),
        ));
    }
    std::fs::read(path).map_err(|error| ImageError::transport("read resource path", &error))
}
