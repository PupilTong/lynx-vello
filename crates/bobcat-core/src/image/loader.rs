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
#[derive(Clone, Debug)]
#[non_exhaustive]
pub(crate) struct LoaderConfig {
    /// The owning template's bundle URL, handed to the fetcher as [`ResourceLocator::base_url`].
    pub(crate) base_url: Option<Url>,
    /// The [`RequestId`] namespace this loader owns.
    pub(crate) request_namespace: u64,
    /// Applies to every transport, not just the buffered one — a hostile or broken host must not
    /// be able to OOM the process through the stream or path branch that the buffered branch
    /// was hardened against.
    pub(crate) max_encoded_bytes: u64,
    pub(crate) decode_cache_bytes: u64,
    pub(crate) header_cache_entries: usize,
    /// In-flight decode cap.
    pub(crate) max_concurrent_decodes: usize,
    /// In-flight *load* cap, acquired before the fetch and held until the decode ends.
    pub(crate) max_concurrent_loads: usize,
    pub(crate) decode: DecodeRequest,
    pub(crate) device_scale: Option<f32>,
    pub(crate) priority: ResourcePriority,
}

impl LoaderConfig {
    /// A configuration for the loader owning `request_namespace`.
    #[must_use]
    pub(crate) fn new(request_namespace: u64) -> Self {
        Self {
            request_namespace,
            ..Self::with_namespace_zero()
        }
    }

    /// Sets the encoded-byte ceiling every transport is held to.
    #[must_use]
    pub(crate) const fn with_max_encoded_bytes(mut self, max_encoded_bytes: u64) -> Self {
        self.max_encoded_bytes = max_encoded_bytes;
        self
    }

    #[must_use]
    pub(crate) const fn with_decode_cache_bytes(mut self, decode_cache_bytes: u64) -> Self {
        self.decode_cache_bytes = decode_cache_bytes;
        self
    }

    #[must_use]
    pub(crate) const fn with_header_cache_entries(mut self, header_cache_entries: usize) -> Self {
        self.header_cache_entries = header_cache_entries;
        self
    }

    #[must_use]
    pub(crate) const fn with_max_concurrent_decodes(
        mut self,
        max_concurrent_decodes: usize,
    ) -> Self {
        self.max_concurrent_decodes = max_concurrent_decodes;
        self
    }

    #[must_use]
    pub(crate) const fn with_max_concurrent_loads(mut self, max_concurrent_loads: usize) -> Self {
        self.max_concurrent_loads = max_concurrent_loads;
        self
    }

    #[must_use]
    pub(crate) fn with_base_url(mut self, base_url: Option<Url>) -> Self {
        self.base_url = base_url;
        self
    }

    #[must_use]
    pub(crate) const fn with_decode(mut self, decode: DecodeRequest) -> Self {
        self.decode = decode;
        self
    }

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

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub(crate) enum ImagePrefetchTarget {
    Encoded(CacheTarget),
    Decoded { target: Option<PixelSize> },
}

/// The loader's mutable state, behind **one** lock.
#[derive(Debug)]
struct LoaderState {
    decodes: DecodeCache,
    headers: HeaderCache,
    resolved: ResolvedMap,
}

/// Fetches, decodes and caches images for one view.
pub(crate) struct ImageLoader {
    fetcher: Arc<dyn ResourceFetcher>,
    decoder: Arc<dyn Decoder>,
    state: Mutex<LoaderState>,
    decode_permits: Arc<Semaphore>,
    load_permits: Arc<Semaphore>,
    sequence: AtomicU64,
    config: LoaderConfig,
    transport: ResourceCapability,
}

impl fmt::Debug for ImageLoader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    pub(crate) fn new(
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
    pub(crate) fn capabilities(&self) -> Capabilities {
        self.decoder.capabilities()
    }

    /// Header-only load: resolve, then cache-or-fetch, then probe.
    pub(crate) async fn header(
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

    /// Full load.
    pub(crate) async fn load(
        &self,
        specifier: &str,
        target: Option<PixelSize>,
        cancel: CancellationToken,
    ) -> Result<DecodeResponse, ImageError> {
        let resolved = self.resolve(specifier, target, cancel.clone()).await?;
        let source = cache_source(&resolved);
        let key = CacheKey::new(Arc::clone(&source), target);

        if let Some(hit) = self.lock().decodes.get(&key) {
            return Ok(hit);
        }

        let Some(load_permit) = self.acquire(&self.load_permits, &cancel).await? else {
            return Err(ImageError::Cancelled);
        };

        let bytes = self.read_bytes(&resolved, cancel.clone()).await?;

        let decoder = Arc::clone(&self.decoder);
        let request = DecodeRequest {
            target_size: target,
            ..self.config.decode
        };
        let Some(permit) = self.acquire(&self.decode_permits, &cancel).await? else {
            return Err(ImageError::Cancelled);
        };

        let decode = tokio::task::spawn_blocking(move || {
            let outcome = decode_bytes(decoder.as_ref(), &bytes, &request);
            drop(permit);
            drop(load_permit);
            outcome
        });

        let Some(joined) = cancel.run_until_cancelled(decode).await else {
            return Err(ImageError::Cancelled);
        };
        let response = joined.map_err(|error| ImageError::decode_join(&error))??;

        if cancel.is_cancelled() {
            return Err(ImageError::Cancelled);
        }

        {
            let mut state = self.lock();
            state.headers.insert(Arc::clone(&source), response.header);
            state.decodes.insert(key, response.clone());
        }
        Ok(response)
    }

    /// Non-blocking decode-cache probe, for a caller already inside a frame commit that must not
    /// await.
    #[must_use]
    pub(crate) fn cached(
        &self,
        specifier: &str,
        target: Option<PixelSize>,
    ) -> Option<DecodedImage> {
        let mut state = self.lock();
        let source = state.resolved.get(&ResolvedKey::new(specifier, target))?;
        state
            .decodes
            .get(&CacheKey::new(source, target))
            .map(|response| response.image)
    }

    /// Non-blocking natural-size probe.
    #[must_use]
    pub(crate) fn cached_header(&self, specifier: &str) -> Option<ImageHeader> {
        let mut state = self.lock();
        let source = state.resolved.get(&ResolvedKey::new(specifier, None))?;
        state.headers.get(&source)
    }

    /// Warms a cache ahead of render.
    pub(crate) async fn prefetch(
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
    pub(crate) fn clear_caches(&self) {
        let mut state = self.lock();
        state.decodes.clear();
        state.headers.clear();
        state.resolved.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LoaderState> {
        self.state.lock().expect("loader state")
    }

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
                    allow_animation: false,
                }),
            },
            percent_decode: true,
        };
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
                    if response.bytes.len() as u64 > limit {
                        return Err(ImageError::EncodedTooLarge { limit });
                    }
                    Ok(response.bytes)
                }
                ResourceCapability::ResourceStream => {
                    let stream = self.fetcher.open_resource(request).await?;
                    let mut bytes = Vec::new();
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
    if bytes.len() as u64 > limit {
        return Err(ImageError::EncodedTooLarge { limit });
    }
    Ok(bytes)
}
