//! Host-injected resource acquisition contracts for bobcat-engine.
//!
//! [`ResourceFetcher`] is the single host boundary for every resource source:
//! images, fonts and media; initial, lazy and frame bundles; runtime scripts
//! and bytecode; packaged assets; and the HTTP transport behind `lynx.fetch`.
//! It deliberately exposes several strongly typed operations instead of
//! flattening all of those protocols into one byte-returning method.
//!
//! This module owns acquisition only. It does not decode images, register
//! fonts, parse bundles, verify template signatures, upload GPU textures, or
//! prescribe memory/disk cache, request coalescing, retry, or timeout policy.
//! Those are responsibilities of the resource pipeline above this contract.

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio_util::sync::CancellationToken;
use url::Url;

/// A dynamically-dispatched asynchronous resource operation.
///
/// Returning a boxed future, rather than declaring `async fn` on
/// [`ResourceFetcher`], keeps the trait usable as `dyn ResourceFetcher`.
pub type ResourceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ResourceError>> + Send + 'a>>;

/// A pull-based resource body backed by Tokio I/O.
///
/// Pull-based reads provide backpressure for large media, HTTP streaming and
/// SSE without requiring the contract to prescribe a buffering task.
pub type ResourceReader = Pin<Box<dyn AsyncRead + Send + 'static>>;

/// The complete host-provided resource acquisition boundary.
///
/// Every request carries a caller-generated [`RequestId`] and a
/// [`CancellationToken`]. An implementation must support concurrent requests,
/// including multiple requests for the same locator, without conflating their
/// cancellation state.
///
/// Cancellation has three equivalent entry points:
///
/// - cancelling the request's token;
/// - dropping an unfinished returned future or [`ResourceReader`];
/// - polling [`Self::cancel`] to completion for detached operations such as prefetch.
///
/// The `cancel` future is best-effort and idempotent: awaiting cancellation of
/// an unknown or completed request succeeds. Once cancellation is observed, an
/// open reader should end promptly with [`std::io::ErrorKind::Interrupted`].
pub trait ResourceFetcher: Send + Sync + 'static {
    /// Reports whether this fetcher can perform an optional operation.
    ///
    /// Calling an operation for which this returns `false` must fail with
    /// [`ResourceErrorKind::UnsupportedOperation`]. Resolution and idempotent
    /// cancellation are mandatory and therefore have no capability flags.
    fn supports(&self, capability: ResourceCapability) -> bool;

    /// Resolves a raw or relative locator without loading its contents.
    ///
    /// Resolution includes template-base URL joining, host URL interception
    /// or CDN rewriting, and locality/cache-key classification. It does not
    /// follow protocol-level HTTP redirects and does not materialize a file.
    fn resolve(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator>;

    /// Loads an encoded resource into bounded contiguous memory.
    ///
    /// The implementation must reject a response larger than
    /// [`BufferedResourceRequest::max_bytes`] with
    /// [`ResourceErrorKind::ResponseTooLarge`]. Image/font/bundle decoding is
    /// explicitly outside this operation.
    fn fetch_resource(
        &self,
        request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, ResourceResponse>;

    /// Opens an encoded resource as a pull-based Tokio reader.
    ///
    /// The future resolves when response metadata and a reader are available,
    /// not when the body has been completely transferred.
    fn open_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceStream>;

    /// Returns or materializes a filesystem path for a resource.
    ///
    /// This is distinct from [`Self::resolve`]: a remote resource may need to
    /// be downloaded to a temporary file, and platforms may provide fallback
    /// paths for native decoders.
    fn fetch_resource_path(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourcePath>;

    /// Performs the HTTP transaction behind the standards-oriented Fetch API.
    ///
    /// Transport, cancellation and policy failures are returned as
    /// [`ResourceError`]. HTTP status codes, including 4xx and 5xx, are
    /// successful [`HttpResponse`] values and must not be converted into
    /// transport errors.
    fn fetch_http(&self, request: HttpRequest) -> ResourceFuture<'_, HttpResponse>;

    /// Warms an encoded-resource cache without decoding the resource.
    ///
    /// The future completes only after the requested cache target has been
    /// populated. Because Rust futures are lazy and this future borrows the
    /// fetcher, fire-and-forget callers must move an `Arc` clone of the fetcher
    /// into a spawned Tokio task and await `prefetch` inside that task. The
    /// request ID remains available for later cancellation.
    fn prefetch(&self, request: PrefetchRequest) -> ResourceFuture<'_, PrefetchReceipt>;

    /// Cancels a request by its caller-generated ID.
    ///
    /// This operation is best-effort and idempotent. It exists in addition to
    /// [`CancellationToken`] for APIs that detach work and retain only an ID.
    fn cancel(&self, request_id: RequestId) -> ResourceFuture<'_, ()>;
}

/// A caller-generated identifier unique within one fetcher instance.
///
/// A fetcher may be shared by several views or runtimes. Each owner therefore
/// supplies a stable namespace and a monotonically increasing sequence; the
/// pair must not be reused while the fetcher remains alive. Namespace
/// allocation belongs to the owners sharing the fetcher; neither
/// [`ResourceFetcher`] nor [`crate::view::LynxView`] coordinates it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId {
    /// View/runtime namespace chosen by the caller.
    pub namespace: u64,
    /// Monotonic request sequence within that namespace.
    pub sequence: u64,
}

/// Scheduling and cancellation state shared by every operation for a request.
#[derive(Clone, Debug)]
pub struct RequestContext {
    /// The ID used for tracing and explicit cancellation.
    pub id: RequestId,
    /// Cooperative cancellation observed by resolution, transfer and readers.
    pub cancellation: CancellationToken,
    /// A transport scheduling hint, not a guarantee of execution order.
    pub priority: ResourcePriority,
}

/// Relative or absolute resource input before host resolution.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResourceLocator {
    /// The original URL, relative specifier, custom scheme or asset name.
    pub specifier: Arc<str>,
    /// Base URL used for relative resolution, normally the owning template.
    pub base_url: Option<Url>,
}

/// Semantic resource categories understood by the runtime.
///
/// The variants intentionally have no wire discriminants: Lynx's core and
/// embedder C APIs disagree on some numeric values, so platform adapters must
/// map by meaning rather than cast integers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceKind {
    /// An application-defined opaque resource.
    Generic,
    /// Encoded raster or animated-image data.
    Image,
    /// Encoded font data.
    Font,
    /// Lottie animation JSON or binary data.
    Lottie,
    /// Encoded audio media.
    Audio,
    /// Encoded video media.
    Video,
    /// SVG markup or encoded SVG data.
    Svg,
    /// The initial page/template bundle.
    Template,
    /// A lazy or dynamic component bundle.
    LazyBundle,
    /// A nested frame bundle.
    Frame,
    /// Server-side-rendered template data.
    SsrData,
    /// The engine's core JavaScript runtime source.
    LynxCoreJs,
    /// An external JavaScript module or chunk.
    ExternalJs,
    /// Externally generated JavaScript bytecode.
    ExternalBytecode,
    /// A packaged application/runtime asset.
    Asset,
    /// Localized text data.
    I18nText,
    /// Graphics subsystem data.
    Graphics,
    /// Theme data.
    Theme,
    /// A URL resolved for [`ResourceFetcher::fetch_http`].
    Fetch,
    /// A forward-compatible embedder-specific resource category.
    Other(Arc<str>),
}

/// A locator plus its semantic category and transport-selection hints.
#[derive(Clone, Debug)]
pub struct ResourceDescriptor {
    /// The unresolved resource location.
    pub locator: ResourceLocator,
    /// The semantic category used for routing and policy.
    pub kind: ResourceKind,
    /// Optional resource-specific hints that do not request decoding.
    pub hints: ResourceHints,
}

/// Optional download/variant-selection hints.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub enum ResourceHints {
    /// No resource-specific hints.
    #[default]
    None,
    /// Hints for choosing an encoded image variant.
    Image(ImageHints),
    /// Hints for a template-like build artifact.
    Bundle(BundleHints),
    /// Hints for byte-range-capable audio/video resources.
    Media(MediaHints),
}

/// Encoded-image selection hints; decoding remains an upper-layer concern.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageHints {
    /// Desired display size in physical pixels, if known.
    pub target_size_px: Option<PixelSize>,
    /// Device scale used by a host/CDN variant selector.
    pub device_scale: Option<f32>,
    /// Whether an animated encoded variant is useful to the caller.
    pub allow_animation: bool,
}

/// A two-dimensional physical-pixel size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelSize {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

/// Template/build-artifact selection hints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleHints {
    /// Bundle entry or component name, when distinct from its locator.
    pub entry_name: Option<Arc<str>>,
}

/// Streaming-media selection hints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaHints {
    /// Optional inclusive byte range requested from the source.
    pub byte_range: Option<ByteRange>,
}

/// An inclusive byte range; `end = None` means through end-of-resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ByteRange {
    /// First requested byte.
    pub start: u64,
    /// Last requested byte, inclusive.
    pub end: Option<u64>,
}

/// Input to [`ResourceFetcher::resolve`].
#[derive(Clone, Debug)]
pub struct ResolveRequest {
    /// Per-request identity, cancellation and priority.
    pub context: RequestContext,
    /// Resource to resolve.
    pub resource: ResourceDescriptor,
    /// Whether the host interceptor should percent-decode the input first.
    pub percent_decode: bool,
}

/// A host-resolved resource locator.
///
/// Its fields remain public so embedders can construct and adapt requests, so
/// receiving one is not proof that [`ResourceFetcher::resolve`] produced it.
/// Fetchers must validate host-constructed values as untrusted input.
#[derive(Clone, Debug)]
pub struct ResolvedLocator {
    /// Original unresolved descriptor.
    pub resource: ResourceDescriptor,
    /// URL after base resolution and host rewriting.
    pub url: Url,
    /// Host rewrite steps, excluding later HTTP protocol redirects.
    pub rewrite_chain: Vec<Url>,
    /// Whether the resolved resource is local to the process/device.
    pub locality: ResourceLocality,
    /// Optional normalized key shared by aliases of the same resource.
    pub cache_key: Option<Arc<str>>,
}

/// Locality classification exposed without forcing an I/O probe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceLocality {
    /// A packaged asset, data URL or local filesystem resource.
    Local,
    /// A network resource.
    Remote,
    /// The resolver cannot determine locality without loading.
    #[default]
    Unknown,
}

/// Input shared by streaming, filesystem-path and prefetch resource loads.
#[derive(Clone, Debug)]
pub struct ResourceRequest {
    /// Per-request identity, cancellation and priority.
    pub context: RequestContext,
    /// Result previously returned by [`ResourceFetcher::resolve`].
    pub resource: ResolvedLocator,
    /// Request headers for URL-backed resources.
    pub headers: HeaderMap,
    /// Cache behavior requested by the upper resource manager.
    pub cache_policy: CachePolicy,
}

/// A resource request that must be materialized in bounded memory.
#[derive(Clone, Debug)]
pub struct BufferedResourceRequest {
    /// Common resolved-resource request data.
    pub request: ResourceRequest,
    /// Hard upper bound for the encoded response body.
    pub max_bytes: u64,
}

/// Metadata shared by every non-Fetch resource response form.
#[derive(Clone, Debug)]
pub struct ResourceMetadata {
    /// The initiating request ID.
    pub request_id: RequestId,
    /// Resolved locator used for the transfer.
    pub resource: ResolvedLocator,
    /// Response headers, if the source protocol provides them.
    pub headers: HeaderMap,
    /// Source-reported encoded length.
    pub content_length: Option<u64>,
    /// Parsed media type, when known independently of response headers.
    pub media_type: Option<Arc<str>>,
    /// Underlying source that produced the response.
    pub source: ResourceSource,
    /// Raw encoded-resource cache outcome.
    pub cache_status: CacheStatus,
    /// Optional transport timing measurements.
    pub timing: ResourceTiming,
}

/// A fully buffered encoded resource.
#[derive(Clone, Debug)]
pub struct ResourceResponse {
    /// Transport and cache metadata.
    pub metadata: ResourceMetadata,
    /// Encoded resource bytes.
    pub bytes: Bytes,
}

/// An encoded resource whose body remains asynchronous.
pub struct ResourceStream {
    /// Metadata available before body completion.
    pub metadata: ResourceMetadata,
    /// Pull-based encoded body.
    pub reader: ResourceReader,
}

impl fmt::Debug for ResourceStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceStream")
            .field("metadata", &self.metadata)
            .field("reader", &"<tokio::io::AsyncRead>")
            .finish()
    }
}

/// Keeps a materialized resource path valid while any clone is retained.
pub trait ResourcePathLease: fmt::Debug + Send + Sync + 'static {}

/// A filesystem-backed encoded resource.
#[derive(Clone, Debug)]
pub struct ResourcePath {
    /// Transport and cache metadata.
    pub metadata: ResourceMetadata,
    /// Primary local path.
    pub path: PathBuf,
    /// Ordered fallback paths supplied by the host.
    pub fallback_paths: Vec<PathBuf>,
    /// Optional owner for temporary-file lifetime and cleanup. `None` means
    /// every returned path remains valid for at least the fetcher's lifetime.
    pub lease: Option<Arc<dyn ResourcePathLease>>,
}

/// Input to the HTTP transport behind `lynx.fetch` and `EventSource`.
///
/// HTTP requests deliberately use a [`ResolvedLocator`], making host URL
/// interception and rewriting an explicit resolve step before every
/// transaction. Implementations may optimize that step internally, but must
/// preserve its observable policy and tracing behavior.
pub struct HttpRequest {
    /// Per-request identity, cancellation and priority.
    pub context: RequestContext,
    /// Resolved URL whose descriptor has [`ResourceKind::Fetch`].
    pub resource: ResolvedLocator,
    /// HTTP method.
    pub method: Method,
    /// HTTP request headers. Repeated header values remain representable.
    pub headers: HeaderMap,
    /// Optional buffered or streaming upload body.
    pub body: HttpRequestBody,
    /// Protocol redirect behavior.
    pub redirect_policy: RedirectPolicy,
    /// HTTP cache behavior.
    pub cache_policy: CachePolicy,
    /// Credential/cookie behavior.
    pub credentials: CredentialsMode,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpRequest")
            .field("context", &self.context)
            .field("resource", &self.resource)
            .field("method", &self.method)
            .field("headers", &self.headers)
            .field("body", &self.body)
            .field("redirect_policy", &self.redirect_policy)
            .field("cache_policy", &self.cache_policy)
            .field("credentials", &self.credentials)
            .finish()
    }
}

/// Buffered or streaming HTTP upload data.
pub enum HttpRequestBody {
    /// No request body.
    Empty,
    /// A finite in-memory request body.
    Bytes(Bytes),
    /// A pull-based upload body.
    Stream(ResourceReader),
}

impl fmt::Debug for HttpRequestBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Bytes(bytes) => formatter
                .debug_tuple("Bytes")
                .field(&format_args!("{} bytes", bytes.len()))
                .finish(),
            Self::Stream(_) => formatter.write_str("Stream(<tokio::io::AsyncRead>)"),
        }
    }
}

/// HTTP response head plus a pull-based body.
pub struct HttpResponse {
    /// The initiating request ID.
    pub request_id: RequestId,
    /// URL after host rewriting and protocol redirects.
    pub final_url: Url,
    /// HTTP status, including non-success statuses.
    pub status: StatusCode,
    /// Protocol-provided reason phrase, if available.
    pub status_text: Option<Arc<str>>,
    /// HTTP response headers.
    pub headers: HeaderMap,
    /// Protocol-level redirect chain, excluding host rewrites.
    pub redirect_chain: Vec<Url>,
    /// Source-reported encoded length.
    pub content_length: Option<u64>,
    /// HTTP cache outcome.
    pub cache_status: CacheStatus,
    /// Optional transport timing measurements.
    pub timing: ResourceTiming,
    /// Pull-based response body, including empty responses.
    pub body: ResourceReader,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpResponse")
            .field("request_id", &self.request_id)
            .field("final_url", &self.final_url)
            .field("status", &self.status)
            .field("status_text", &self.status_text)
            .field("headers", &self.headers)
            .field("redirect_chain", &self.redirect_chain)
            .field("content_length", &self.content_length)
            .field("cache_status", &self.cache_status)
            .field("timing", &self.timing)
            .field("body", &"<tokio::io::AsyncRead>")
            .finish()
    }
}

/// A raw encoded-resource prefetch request.
#[derive(Clone, Debug)]
pub struct PrefetchRequest {
    /// The same resource request shape used for foreground loading.
    pub request: ResourceRequest,
    /// Encoded-data cache to warm.
    pub target: CacheTarget,
    /// Hard upper bound for bytes transferred into the cache.
    pub max_bytes: u64,
}

/// Confirmation that a prefetch reached its requested cache target.
#[derive(Clone, Debug)]
pub struct PrefetchReceipt {
    /// The request that may later be passed to [`ResourceFetcher::cancel`].
    pub request_id: RequestId,
    /// Resolved resource that was cached.
    pub resource: ResolvedLocator,
    /// Final cache outcome.
    pub cache_status: CacheStatus,
    /// Encoded bytes transferred, when known.
    pub transferred_bytes: Option<u64>,
}

/// An optional operation queryable through [`ResourceFetcher::supports`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceCapability {
    /// [`ResourceFetcher::fetch_resource`].
    BufferedResource,
    /// [`ResourceFetcher::open_resource`].
    ResourceStream,
    /// [`ResourceFetcher::fetch_resource_path`].
    ResourcePath,
    /// [`ResourceFetcher::fetch_http`].
    Http,
    /// A streaming [`HttpRequestBody`] upload.
    StreamingUpload,
    /// [`ResourceFetcher::prefetch`].
    Prefetch,
}

/// Scheduling priority supplied by the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourcePriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

/// Cache behavior shared by resources and HTTP requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CachePolicy {
    #[default]
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}

/// Encoded-data cache target for prefetch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheTarget {
    #[default]
    Automatic,
    Memory,
    Disk,
    MemoryAndDisk,
}

/// HTTP redirect policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RedirectPolicy {
    Follow {
        /// Maximum protocol redirects before failing.
        max_hops: u8,
    },
    Manual,
    Error,
}

/// Fetch credential/cookie policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CredentialsMode {
    Omit,
    #[default]
    SameOrigin,
    Include,
}

/// Origin of encoded resource bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceSource {
    Network,
    FileSystem,
    PackagedAsset,
    DataUrl,
    MemoryCache,
    DiskCache,
    Custom,
}

/// Cache result reported by a fetch or prefetch operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheStatus {
    /// The operation did not consult a cache.
    #[default]
    NotApplicable,
    Miss,
    HitMemory,
    HitDisk,
    Revalidated,
    Bypassed,
}

/// Optional durations recorded by a transport implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceTiming {
    /// URL/base resolution and host rewrite time.
    pub resolve: Option<Duration>,
    /// DNS, socket and TLS connection time.
    pub connect: Option<Duration>,
    /// Time from request start until response headers/first body byte.
    pub time_to_first_byte: Option<Duration>,
    /// Encoded-body transfer time.
    pub transfer: Option<Duration>,
    /// Total operation time through the latest completed phase.
    pub total: Option<Duration>,
}

/// Stable resource failure details shared by every operation.
#[derive(Clone, Debug, Error)]
#[error("{kind:?} during {phase:?}: {message}")]
pub struct ResourceError {
    /// Request associated with the failure, if one had been assigned.
    pub request_id: Option<RequestId>,
    /// Stable failure category.
    pub kind: ResourceErrorKind,
    /// Operation phase in which the failure occurred.
    pub phase: ResourceErrorPhase,
    /// Original or resolved locator associated with the failure.
    pub locator: Option<Arc<str>>,
    /// HTTP status associated with a non-Fetch resource failure.
    pub status: Option<StatusCode>,
    /// Human-readable diagnostic text; callers must not branch on it.
    pub message: Arc<str>,
    /// Advisory only; this trait never performs automatic retries.
    pub retry: RetryAdvice,
}

/// Stable resource failure categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceErrorKind {
    Cancelled,
    InvalidRequest,
    InvalidUrl,
    UnsupportedScheme,
    UnsupportedKind,
    UnsupportedOperation,
    NotFound,
    PermissionDenied,
    PolicyDenied,
    Dns,
    Connect,
    Tls,
    Protocol,
    RedirectLoop,
    TooManyRedirects,
    RequestBody,
    ResponseBody,
    Io,
    IntegrityMismatch,
    ResponseTooLarge,
    Unavailable,
    Other,
}

/// The operation phase that produced a [`ResourceError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceErrorPhase {
    Resolve,
    Open,
    Connect,
    SendRequest,
    ReceiveHeaders,
    ReadBody,
    MaterializePath,
    Prefetch,
    Cancel,
}

/// Whether an upper layer may choose to retry a failed operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetryAdvice {
    #[default]
    Never,
    Immediate,
    After(Duration),
}

#[cfg(test)]
mod tests {
    use super::ResourceFetcher;

    fn accepts_object_safe_trait(_: Option<&dyn ResourceFetcher>) {}

    #[test]
    fn resource_fetcher_is_object_safe() {
        accepts_object_safe_trait(None);
    }
}
