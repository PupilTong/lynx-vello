//! Host-injected resource acquisition contracts for Bobcat.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};
use thiserror::Error;
use tokio::io::AsyncRead;
use url::Url;

use crate::style::PreparsedStyleSheet;

/// A resource operation polled directly on the thread that called the fetcher.
///
/// During view startup that owner is `bobcat-main`, which deliberately has no
/// ambient Tokio runtime or IO reactor. Implementations may move actual file or
/// network IO onto their own executor or worker threads and wake this future,
/// but the returned future itself must be executor-neutral and must not assume
/// a caller-provided runtime.
pub type ResourceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ResourceError>> + Send + 'a>>;

pub type ResourceReader = Pin<Box<dyn AsyncRead + Send + 'static>>;

/// Host-owned resource policy whose startup calls and continuations run on
/// `bobcat-main`.
///
/// Implementations own any executor or reactor their IO requires; Bobcat only
/// polls the returned [`ResourceFuture`] until it is woken and ready.
pub trait ResourceFetcher: Send + Sync + 'static {
    fn supports_capability(&self, capability: ResourceCapability) -> bool;

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator>;

    fn fetch_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceResponse>;

    /// Loads a stylesheet in whichever form this host has it.
    ///
    /// The default answers from [`Self::fetch_resource`] as
    /// [`StyleSheetPayload::Text`], which is correct for any host that only
    /// moves bytes — a browser embedder cannot decode a `.web.bundle` at all.
    /// A host that reports [`ResourceCapability::PreparsedStyleSheet`]
    /// overrides this to return [`StyleSheetPayload::Preparsed`].
    fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> ResourceFuture<'_, StyleSheetResponse> {
        fetch_style_sheet_as_text(self, request)
    }

    fn fetch_http(&self, request: HttpRequest) -> ResourceFuture<'_, HttpResponse>;
}

/// Answers a stylesheet request from [`ResourceFetcher::fetch_resource`] as
/// [`StyleSheetPayload::Text`] — the body of the default
/// [`ResourceFetcher::fetch_style_sheet`].
///
/// An override that answers only *some* requests pre-parsed calls this for the
/// rest, rather than re-implementing the byte path.
pub fn fetch_style_sheet_as_text<F>(
    fetcher: &F,
    request: ResourceRequest,
) -> ResourceFuture<'_, StyleSheetResponse>
where
    F: ResourceFetcher + ?Sized,
{
    Box::pin(async move {
        let response = fetcher.fetch_resource(request).await?;
        Ok(StyleSheetResponse {
            metadata: response.metadata,
            payload: StyleSheetPayload::Text(response.bytes),
        })
    })
}

/// A caller-generated identifier unique within one fetcher instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId {
    pub namespace: u64,
    pub sequence: u64,
}

/// Scheduling state shared by every operation for a request.
#[derive(Clone, Debug)]
pub struct RequestContext {
    pub id: RequestId,
    pub priority: ResourcePriority,
}

/// Relative or absolute resource input before host resolution.
#[derive(Clone, Debug)]
pub struct ResourceDescriptor {
    pub specifier: Arc<str>,
    pub base_url: Option<Url>,
}

/// Input for resolving a resource descriptor before loading it.
#[derive(Clone, Debug)]
pub struct ResolveRequest {
    pub context: RequestContext,
    pub resource: ResourceDescriptor,
    pub percent_decode: bool,
}

/// A host-resolved resource locator.
#[derive(Clone, Debug)]
pub struct ResolvedLocator {
    pub resource: ResourceDescriptor,
    pub url: Url,
    pub rewrite_chain: Vec<Url>,
    pub locality: ResourceLocality,
    pub cache_key: Option<Arc<str>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceLocality {
    Local,
    Remote,
    #[default]
    Unknown,
}

/// A resolved resource load.
///
/// It carries no response-size budget. The fetcher owns any memory limit for
/// the response it materializes.
#[derive(Clone, Debug)]
pub struct ResourceRequest {
    pub context: RequestContext,
    pub resource: ResolvedLocator,
    pub headers: HeaderMap,
    pub cache_policy: CachePolicy,
}

/// Metadata shared by every non-Fetch resource response form.
#[derive(Clone, Debug)]
pub struct ResourceMetadata {
    pub request_id: RequestId,
    pub resource: ResolvedLocator,
    pub headers: HeaderMap,
    pub content_length: Option<u64>,
    pub media_type: Option<Arc<str>>,
    pub source: ResourceSource,
    pub cache_status: CacheStatus,
    pub timing: ResourceTiming,
}

/// A fully buffered encoded resource.
#[derive(Clone, Debug)]
pub struct ResourceResponse {
    pub metadata: ResourceMetadata,
    pub bytes: Bytes,
}

/// A stylesheet in whichever of the two accepted forms the host has.
///
/// A host that only moves bytes returns [`StyleSheetPayload::Text`]; one that
/// already decoded a `.web.bundle`'s pre-parsed CSS returns
/// [`StyleSheetPayload::Preparsed`], which skips the CSS parser.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum StyleSheetPayload {
    /// UTF-8 CSS source text.
    Text(Bytes),
    /// A stylesheet the host parsed before the engine saw it.
    Preparsed(Arc<PreparsedStyleSheet>),
}

/// A fully buffered stylesheet response.
#[derive(Clone, Debug)]
pub struct StyleSheetResponse {
    pub metadata: ResourceMetadata,
    pub payload: StyleSheetPayload,
}

/// Input to the HTTP transport behind `lynx.fetch` and `EventSource`.
pub struct HttpRequest {
    pub context: RequestContext,
    pub resource: ResolvedLocator,
    pub method: Method,
    pub headers: HeaderMap,
    pub body: HttpRequestBody,
    pub redirect_policy: RedirectPolicy,
    pub cache_policy: CachePolicy,
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

pub enum HttpRequestBody {
    Empty,
    Bytes(Bytes),
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
    pub request_id: RequestId,
    pub final_url: Url,
    pub status: StatusCode,
    pub status_text: Option<Arc<str>>,
    pub headers: HeaderMap,
    pub redirect_chain: Vec<Url>,
    pub content_length: Option<u64>,
    pub cache_status: CacheStatus,
    pub timing: ResourceTiming,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceCapability {
    BufferedResource,
    /// Answering a stylesheet request with a host-decoded
    /// [`PreparsedStyleSheet`] instead of CSS text.
    PreparsedStyleSheet,
    Http,
    StreamingUpload,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourcePriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RedirectPolicy {
    Follow { max_hops: u8 },
    Manual,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CredentialsMode {
    Omit,
    #[default]
    SameOrigin,
    Include,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheStatus {
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
    pub resolve: Option<Duration>,
    pub connect: Option<Duration>,
    pub time_to_first_byte: Option<Duration>,
    pub transfer: Option<Duration>,
    pub total: Option<Duration>,
}

/// Stable resource failure details shared by every operation.
#[derive(Clone, Debug, Error)]
#[error("{kind:?} during {phase:?}: {message}")]
pub struct ResourceError {
    pub request_id: Option<RequestId>,
    pub kind: ResourceErrorKind,
    pub phase: ResourceErrorPhase,
    pub locator: Option<Arc<str>>,
    pub status: Option<StatusCode>,
    pub message: Arc<str>,
    pub retry: RetryAdvice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceErrorKind {
    InvalidRequest,
    InvalidUrl,
    UnsupportedScheme,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceErrorPhase {
    Resolve,
    Open,
    Connect,
    SendRequest,
    ReceiveHeaders,
    ReadBody,
}

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
