//! Host-injected resource acquisition contracts for Bobcat.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use thiserror::Error;
use url::Url;

use crate::style::PreparsedStyleSheet;

/// The host's whole resource system: bytes, stylesheets and images.
///
/// Owned by the painter, which is the thread that constructed the view, and
/// never reachable from `bobcat-main` — every resource the document needs is
/// asked for by message. It is therefore free to be neither `Send` nor `Sync`
/// and to hold `Rc`, `RefCell` or browser objects directly.
///
/// The two halves have deliberately different shapes, because their callers
/// do. Bytes and stylesheets are awaited off the frame path, so they are
/// futures. Images are named synchronously, loaded on the host's own
/// concurrency, and reported through [`ImageReports`](dom::ImageReports); the pixels are then
/// read back synchronously by [`dom::FrameImages::read`] during composition,
/// which cannot suspend. That read may block, and after a successful load it
/// must not miss — see [`dom::FrameImages`].
///
/// Every fetch is polled directly on the painter's thread, which has no
/// ambient Tokio runtime or IO reactor — the CLI drives the whole of
/// construction with `pollster`, which supplies neither. Implementations own
/// any executor or reactor their IO requires: move the real file or network
/// work onto it, wake the future, and Bobcat polls until it is ready. What
/// the future must not do is assume a runtime its caller never promised.
#[expect(
    async_fn_in_trait,
    reason = "the absent `Send` is the point: a resource system is thread-bound, \
              and its futures are polled only on the painter that owns it"
)]
pub trait ResourceFetcher: dom::FrameImages {
    fn supports_capability(&self, capability: ResourceCapability) -> bool;

    async fn resolve_locator(
        &self,
        request: ResolveRequest,
    ) -> Result<ResolvedLocator, ResourceError>;

    async fn fetch_resource(
        &self,
        request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError>;

    /// Loads a stylesheet in whichever form this host has it.
    ///
    /// The default answers from [`Self::fetch_resource`] as
    /// [`StyleSheetPayload::Text`], which is correct for any host that only
    /// moves bytes — a browser embedder cannot decode a `.web.bundle` at all.
    /// A host that reports [`ResourceCapability::PreparsedStyleSheet`]
    /// overrides this to return [`StyleSheetPayload::Preparsed`].
    async fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> Result<StyleSheetResponse, ResourceError> {
        fetch_style_sheet_as_text(self, request).await
    }

    /// Names `source` and begins loading it. Non-blocking.
    ///
    /// Idempotent and single-flight: repeated or concurrent requests for one
    /// source join one load, and a request for an already-loaded source
    /// starts nothing. The engine asks for a source exactly once per
    /// document, keyed by the raw string the page wrote — two specifiers a
    /// host canonicalises to one resource are simply asked for twice.
    ///
    /// For every source it is asked for, a host eventually calls exactly one
    /// of [`ImageReports::loaded`](dom::ImageReports::loaded) or
    /// [`ImageReports::failed`](dom::ImageReports::failed), unless the view is
    /// torn down first. Reporting and asking for the turn that drains the
    /// report are both the host's, and both happen on this thread.
    ///
    /// The default serves nothing, which is what a host with no image support
    /// wants: a source is asked for once and then never drawn.
    fn request_image(&self, _source: &str) {}

    /// The sources the frame just encoded, deduplicated in paint order.
    ///
    /// Advisory: it informs residency and nothing else, and a host that
    /// ignores it is still correct. Called once per resolve pass.
    fn retain_images(&self, _frame: &[Arc<str>]) {}

    /// The host's own moment in every painter turn, on this thread, before
    /// the turn reads the reports queued so far.
    ///
    /// A host whose loads finish somewhere else — a decode thread, a
    /// browser worker — forwards each completion into its
    /// [`ImageReports`](dom::ImageReports) here, so a load that completed
    /// between turns is reported in the next one whether or not that turn
    /// requested or resolved anything. Waking the painter for that turn is
    /// still the host's, through the wakeup it gave the view. A host that
    /// reports inline has nothing to do, and the default does nothing.
    fn service_images(&self) {}
}

/// A shared handle serves whatever it points at.
///
/// The painter owns its resource system by value; an embedder whose registry
/// outlives the view hands in an [`Rc`] of it instead. This is what joins the
/// two without a per-embedder forwarding wrapper. `Rc` rather than `Arc`
/// because nothing on this path crosses a thread — an atomic count here would
/// be paid on every clone and never used.
///
/// It is the right handle for a registry that answers reads and fetches, and
/// the wrong one for a registry that *reports* — an [`ImageReports`](dom::ImageReports)
/// belongs to one view, and a handle shared across views has nowhere to put
/// more than one. A host that loads images asynchronously returns a per-view
/// value from the builder [`create_lynx_view`](crate::LynxGroup::create_lynx_view) takes,
/// holding this handle plus that view's reports.
impl<T: ResourceFetcher + ?Sized> ResourceFetcher for Rc<T> {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        (**self).supports_capability(capability)
    }

    async fn resolve_locator(
        &self,
        request: ResolveRequest,
    ) -> Result<ResolvedLocator, ResourceError> {
        (**self).resolve_locator(request).await
    }

    async fn fetch_resource(
        &self,
        request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError> {
        (**self).fetch_resource(request).await
    }

    async fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> Result<StyleSheetResponse, ResourceError> {
        (**self).fetch_style_sheet(request).await
    }

    fn request_image(&self, source: &str) {
        (**self).request_image(source);
    }

    fn retain_images(&self, frame: &[Arc<str>]) {
        (**self).retain_images(frame);
    }

    fn service_images(&self) {
        (**self).service_images();
    }
}

/// Answers a stylesheet request from [`ResourceFetcher::fetch_resource`] as
/// [`StyleSheetPayload::Text`] — the body of the default
/// [`ResourceFetcher::fetch_style_sheet`].
///
/// An override that answers only *some* requests pre-parsed calls this for the
/// rest, rather than re-implementing the byte path.
pub async fn fetch_style_sheet_as_text<F>(
    fetcher: &F,
    request: ResourceRequest,
) -> Result<StyleSheetResponse, ResourceError>
where
    F: ResourceFetcher + ?Sized,
{
    let response = fetcher.fetch_resource(request).await?;
    Ok(StyleSheetResponse {
        metadata: response.metadata,
        payload: StyleSheetPayload::Text(response.bytes),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceCapability {
    BufferedResource,
    /// Answering a stylesheet request with a host-decoded
    /// [`PreparsedStyleSheet`] instead of CSS text.
    PreparsedStyleSheet,
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

/// A host that answers nothing, ever: every fetch is a future that never
/// completes and no image is ever readable.
///
/// The painter owns a resource system unconditionally, so a test that is not
/// about resources still needs one to name.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct NeverAnswers;

#[cfg(test)]
impl dom::FrameImages for NeverAnswers {
    fn read(
        &self,
        _source: &str,
        _hint: dom::ImageSizeHint,
    ) -> Option<dom::vello::peniko::ImageData> {
        None
    }
}

#[cfg(test)]
impl ResourceFetcher for NeverAnswers {
    fn supports_capability(&self, _capability: ResourceCapability) -> bool {
        false
    }

    async fn resolve_locator(
        &self,
        _request: ResolveRequest,
    ) -> Result<ResolvedLocator, ResourceError> {
        std::future::pending().await
    }

    async fn fetch_resource(
        &self,
        _request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError> {
        std::future::pending().await
    }
}
