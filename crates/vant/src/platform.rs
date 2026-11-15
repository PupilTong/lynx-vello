//! The capabilities an embedder provides: a transport ([`Fetch`]),
//! persistent bytes ([`Store`], or [`AsyncStore`] for the artifact tier of a
//! host whose storage answers asynchronously) and platform pixels
//! ([`ImageDecoder`]), bundled as one [`Platform`].
//!
//! They are shaped after the web platform's Fetch and Cache APIs. [`Store`]
//! adds what this engine needs and the Cache API lacks: synchronous storage,
//! a body mapped read-only, and an LRU signal. [`AsyncStore`] is the subset
//! of [`Store`] the artifact tier uses, answered asynchronously, as the
//! Cache API answers. Every policy decision — what to cache, for how long,
//! what a request sends — stays in this crate; an implementation only moves
//! bytes (`docs/resource-cache-design.md`).
//!
//! Capabilities are traits with associated types, never `dyn`, so a host
//! that names its platform pays no dispatch for it.

use std::future::Future;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::HeaderMap;
use url::Url;

use crate::CachePolicy;
use crate::error::{Failure, ResourceErrorKind, ResourceErrorPhase};
use crate::image::{Bitmap, DecodeError};
use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;

/// The thread bounds every capability carries: `Send + Sync + 'static` on
/// native targets, where the resource system runs on a pool of threads.
///
/// On wasm32 the bound is `'static` alone. The browser decoder holds a
/// `MessagePort`, which is neither `Send` nor `Sync`, and there the whole
/// resource system lives on the Render Worker and never crosses a thread,
/// so the bounds would only rule out the decoder the target needs.
#[cfg(not(target_arch = "wasm32"))]
pub trait Threaded: Send + Sync + 'static {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + 'static> Threaded for T {}

/// The thread bounds every capability carries: `'static` alone on wasm32,
/// where the resource system never leaves the Render Worker and the browser
/// decoder holds a `MessagePort` that is neither `Send` nor `Sync`.
#[cfg(target_arch = "wasm32")]
pub trait Threaded: 'static {}

#[cfg(target_arch = "wasm32")]
impl<T: 'static> Threaded for T {}

/// The bound on the futures an [`AsyncStore`] answers with: `Send` on native
/// targets, where the tier that awaits them runs as a task of a multi-thread
/// pool, and nothing on wasm32, where a browser's Promises are bound to the
/// thread that made them.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// The bound on the futures an [`AsyncStore`] answers with: nothing on
/// wasm32, where a browser's Promises are bound to the thread that made them.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// One request a [`Fetch`] moves the bytes for.
#[derive(Clone, Debug)]
pub struct Request {
    pub url: Url,
    /// The load's cache policy. A transport that applies HTTP cache
    /// semantics itself ([`Fetch::owns_http_cache`]) maps it onto its own
    /// cache mode; any other has already been answered by the byte tier.
    pub policy: CachePolicy,
    /// Caller headers plus any conditional headers the byte tier added.
    pub headers: HeaderMap,
    pub timeout: Duration,
    /// The largest body the transport accepts before failing the request.
    pub max_body: usize,
    pub max_redirects: u32,
    pub user_agent: String,
}

/// What a [`Fetch`] answered.
#[derive(Clone, Debug)]
pub struct Response {
    /// The final URL after redirects.
    pub url: Url,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// The transport: serves `http`, `https` and, natively, `file:` URLs.
/// Registered contents and `data:` URLs are answered by this crate before
/// any transport is asked.
pub trait Fetch: Threaded {
    /// Moves the bytes, blocking the calling thread. `None` from a transport
    /// that cannot block (the browser), which tells the image pipeline to
    /// retain encoded bytes: what it fetched cannot be read back without a
    /// new request.
    fn fetch_blocking(&self, request: &Request) -> Option<Result<Response, Failure>>;

    /// Moves the bytes asynchronously. The default answers with
    /// [`fetch_blocking`](Self::fetch_blocking) when it is polled, and fails
    /// for a transport that cannot block; the browser overrides it with
    /// `fetch()`.
    fn fetch(&self, request: Request) -> impl Future<Output = Result<Response, Failure>> {
        async move {
            self.fetch_blocking(&request).unwrap_or_else(|| {
                Err(Failure::new(
                    ResourceErrorKind::UnsupportedOperation,
                    ResourceErrorPhase::Open,
                    "this transport cannot fetch synchronously",
                ))
            })
        }
    }

    /// Whether the transport applies HTTP cache semantics itself (the
    /// browser's `fetch` does, through the request's cache mode). When
    /// `true`, no byte tier runs in front of it.
    fn owns_http_cache(&self) -> bool;

    /// Reads a `file:` URL's contents, mapped read-only, for bytes a consumer
    /// retains for a view's lifetime (fonts). `None` where files cannot be
    /// mapped. Mapped bytes come back as the same `Bytes` for one unchanged
    /// file, process-wide.
    fn map_file(&self, url: &Url) -> Option<Result<Bytes, Failure>> {
        let _ = url;
        None
    }
}

/// What a [`Store`] keeps beside a body. `record` is the tier's own
/// serialisation (an HTTP response record, an artifact header); the Store
/// never reads it.
#[derive(Clone, Debug)]
pub struct Meta {
    /// The full key, so a Store that names entries by a hash can detect a
    /// collision: every tier checks it on every read and treats a mismatch
    /// as a miss.
    pub key: String,
    /// The body's length in bytes.
    pub len: u64,
    /// When the entry was last used, as far as the Store was last told.
    pub last_used: SystemTime,
    pub record: serde_json::Value,
}

/// How much a [`Store`] holds, and how much it may hold.
#[derive(Clone, Copy, Debug, Default)]
pub struct Estimate {
    pub used: u64,
    /// The most the host lets the Store hold, when it says (Android's
    /// `getCacheQuotaBytes`, a browser origin's quota).
    pub quota: Option<u64>,
}

/// How the host may take a [`Store`]'s contents away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Volatility {
    /// Only this engine deletes entries.
    Durable,
    /// The OS may purge everything while no process of the app runs (iOS
    /// `Library/Caches`, macOS caches).
    PurgedWhenIdle,
    /// Entries may disappear at any moment, singly or all at once (Android
    /// `cacheDir`, browser storage under pressure or Safari's 7-day cap).
    EvictableAnytime,
}

/// Why a [`Store`] operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store IO failed: {0}")]
    Io(#[from] std::io::Error),
    /// An entry's halves disagree or cannot be read back.
    #[error("store entry is corrupt: {0}")]
    Corrupt(String),
    #[error("store failed: {0}")]
    Other(String),
}

/// Persistent bytes: one body and one [`Meta`] per key, answered
/// synchronously (natively, a directory).
///
/// The tiers built on a Store treat what it answers as the truth and their
/// own index as a hint, so a Store may lose entries at any time.
pub trait Store: Threaded {
    /// Every entry present, for rebuilding the index. Broken halves are
    /// deleted on the way.
    fn scan(&self) -> Result<Vec<Meta>, StoreError>;
    fn get(&self, key: &str) -> Result<Option<(Meta, Bytes)>, StoreError>;
    fn get_meta(&self, key: &str) -> Result<Option<Meta>, StoreError>;
    /// Atomic per entry: a crash leaves the old entry or none, never a
    /// truncated one.
    fn put(&self, meta: &Meta, body: &[u8]) -> Result<(), StoreError>;
    /// Replaces the record of an existing entry (a 304 refresh).
    fn put_meta(&self, meta: &Meta) -> Result<(), StoreError>;
    fn delete(&self, key: &str) -> Result<(), StoreError>;
    /// The LRU signal; an implementation also moves the file's mtime so a
    /// host that evicts by mtime (Android) agrees with this crate's order.
    fn touch(&self, key: &str, now: SystemTime) -> Result<(), StoreError>;
    /// The body mapped read-only, for retained bytes; `None` where not
    /// supported.
    fn map_body(&self, key: &str) -> Option<Bytes> {
        let _ = key;
        None
    }
    fn estimate(&self) -> Estimate;
    fn volatility(&self) -> Volatility;
}

/// The [`Platform::Store`] or [`Platform::AsyncStore`] of a host without
/// one. It has no values, so [`Platform::into_parts`] can never hand one
/// over.
#[derive(Clone, Copy, Debug)]
pub enum NoStore {}

impl Store for NoStore {
    fn scan(&self) -> Result<Vec<Meta>, StoreError> {
        match *self {}
    }

    fn get(&self, _key: &str) -> Result<Option<(Meta, Bytes)>, StoreError> {
        match *self {}
    }

    fn get_meta(&self, _key: &str) -> Result<Option<Meta>, StoreError> {
        match *self {}
    }

    fn put(&self, _meta: &Meta, _body: &[u8]) -> Result<(), StoreError> {
        match *self {}
    }

    fn put_meta(&self, _meta: &Meta) -> Result<(), StoreError> {
        match *self {}
    }

    fn delete(&self, _key: &str) -> Result<(), StoreError> {
        match *self {}
    }

    fn touch(&self, _key: &str, _now: SystemTime) -> Result<(), StoreError> {
        match *self {}
    }

    fn map_body(&self, _key: &str) -> Option<Bytes> {
        match *self {}
    }

    fn estimate(&self) -> Estimate {
        match *self {}
    }

    fn volatility(&self) -> Volatility {
        match *self {}
    }
}

/// Persistent bytes answered asynchronously, for the artifact tier
/// ([`AsyncArtifactCache`](crate::artifact::AsyncArtifactCache)) of a host
/// whose storage is asynchronous.
///
/// Modelled on the web Cache API (`caches.open(name)`): an entry is one
/// request key and one response, `scan` is `matchAll`, `get` is `match`,
/// `put` is `put` and `delete` is `delete`. Only what the artifact tier uses
/// is here; the HTTP byte tier reads synchronously and stays on [`Store`].
///
/// The contracts are [`Store`]'s: a `put` is atomic per entry, the tier
/// checks [`Meta::key`] on every read and treats a mismatch as a miss, and
/// entries may vanish at any time, so the tier treats what the store answers
/// as the truth and its own index as a hint.
pub trait AsyncStore: Threaded {
    /// Every entry present, for rebuilding the index. An entry whose
    /// metadata cannot be read back is deleted on the way.
    fn scan(&self) -> impl Future<Output = Result<Vec<Meta>, StoreError>> + MaybeSend;
    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<(Meta, Bytes)>, StoreError>> + MaybeSend;
    /// Atomic per entry: a failure leaves the old entry or none, never a
    /// truncated one.
    fn put(
        &self,
        meta: &Meta,
        body: Bytes,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    /// The LRU signal: records `now` as the entry's `last_used`. A key with
    /// no entry is left absent.
    fn touch(
        &self,
        key: &str,
        now: SystemTime,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn estimate(&self) -> impl Future<Output = Estimate> + MaybeSend;
    fn volatility(&self) -> Volatility;
}

impl AsyncStore for NoStore {
    fn scan(&self) -> impl Future<Output = Result<Vec<Meta>, StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn get(
        &self,
        _key: &str,
    ) -> impl Future<Output = Result<Option<(Meta, Bytes)>, StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn put(
        &self,
        _meta: &Meta,
        _body: Bytes,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn delete(&self, _key: &str) -> impl Future<Output = Result<(), StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn touch(
        &self,
        _key: &str,
        _now: SystemTime,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn estimate(&self) -> impl Future<Output = Estimate> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn volatility(&self) -> Volatility {
        match *self {}
    }
}

/// Platform pixels: decodes an encoded image to a [`Bitmap`].
pub trait ImageDecoder: Threaded {
    /// Decodes at most `max` large, blocking the calling thread. A decoder
    /// that cannot block (the browser's main-thread `Image`) answers
    /// [`DecodeError::Unavailable`], and is reached through
    /// [`decode`](Self::decode) instead.
    fn decode_blocking(
        &self,
        bytes: &[u8],
        format: ImageFormat,
        header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError>;

    /// Decodes asynchronously. The default runs
    /// [`decode_blocking`](Self::decode_blocking) when it is polled.
    fn decode(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> impl Future<Output = Result<Bitmap, DecodeError>> {
        async move { self.decode_blocking(&bytes, format, header, max) }
    }

    /// Whether the decoder can run at all, reported once at startup rather
    /// than once per image.
    fn available(&self) -> Result<(), DecodeError>;
}

/// A [`Store`] with the byte budget the tier built over it may use. The tier
/// clamps the budget further to [`Store::estimate`]'s quota, so `u64::MAX`
/// means the quota, or unbounded without one.
#[derive(Clone, Copy, Debug)]
pub struct Tier<S> {
    pub store: S,
    pub budget: u64,
}

/// The derived-artifact tier's store, of either kind: a synchronous
/// [`Store`], driven by an [`ArtifactCache`](crate::artifact::ArtifactCache),
/// or an [`AsyncStore`], driven by an
/// [`AsyncArtifactCache`](crate::artifact::AsyncArtifactCache).
#[derive(Clone, Copy, Debug)]
pub enum ArtifactStore<S, A> {
    Blocking(S),
    Async(A),
}

/// What a [`Platform`] is taken apart into, once, when the resource system
/// is built.
#[derive(Debug)]
pub struct Parts<F, S, A, D> {
    pub fetch: F,
    pub decoder: D,
    /// The HTTP byte tier's Store; `None` for a host with no persistent
    /// tier. Always a synchronous [`Store`]: the byte tier restores a body
    /// inside a synchronous call and maps retained bodies.
    pub bytes: Option<Tier<S>>,
    /// The derived-artifact tier's store (the code cache): a namespace of its
    /// own, never the byte tier's. `None` keeps artifacts in memory only.
    pub artifacts: Option<Tier<ArtifactStore<S, A>>>,
}

/// The bundle an embedder hands over: one transport, one decoder, and up to
/// two stores, each with its budget.
///
/// The parts are handed over by value, because the tiers built on them own
/// them: a [`ByteCache`](crate::byte_cache::ByteCache) takes its Store, an
/// [`ArtifactCache`](crate::artifact::ArtifactCache) or an
/// [`AsyncArtifactCache`](crate::artifact::AsyncArtifactCache) its own.
pub trait Platform: 'static {
    type Fetch: Fetch;
    /// [`NoStore`] for a host with no synchronous persistent tier.
    type Store: Store;
    /// [`NoStore`] for a host with no asynchronous store.
    type AsyncStore: AsyncStore;
    type Decoder: ImageDecoder;

    /// Takes the platform apart once, when the resource system is built.
    fn into_parts(self) -> Parts<Self::Fetch, Self::Store, Self::AsyncStore, Self::Decoder>;
}
