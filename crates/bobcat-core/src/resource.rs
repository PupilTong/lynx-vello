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
/// Source requests are non-blocking: the fetcher resolves the URL, loads and
/// validates UTF-8 (or returns a pre-parsed sheet), then consumes the concrete
/// [`SourceCompletion`] to answer whoever asked, without naming them. It owns
/// any executor its IO
/// needs; core retains and polls no resource future. Images are reported through
/// [`ImageReports`](dom::ImageReports), then read during composition.
///
/// The lower-level async byte API is available to embedders; core startup uses
/// only [`Self::request_source`]. Its futures require the caller's own executor.
#[expect(
    async_fn_in_trait,
    reason = "embedder byte operations may be thread-bound"
)]
pub trait ResourceFetcher: dom::FrameImages {
    /// Begins one source load without blocking the painter. Main requests each
    /// stylesheet in cascade order, then the entry, with one outstanding startup
    /// source. Worker scripts can be requested concurrently after entry begins.
    ///
    /// Consume `completion` with the result, or retain it until the load finishes.
    /// Dropping it unanswered reports a failure unless the view has ended.
    /// Check [`SourceCompletion::is_cancelled`] before starting queued work and
    /// after IO; a cancelled load no longer needs to decode or deliver a result.
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion);

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
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        (**self).request_source(request, completion);
    }

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

/// One source requested by the document owner. Resolution belongs to the fetcher.
#[derive(Debug)]
pub enum SourceRequest {
    StyleSheet(String),
    Entry(String),
    /// A worker script resolved against the creating view's entry URL.
    /// Complete with `LoadedSource::Entry`; the result goes to its worker.
    Worker {
        specifier: String,
        base_url: String,
    },
    /// A normalized module URL, loaded after an import discovers it.
    Module(String),
}

/// A stylesheet ready to mount. The fetcher has already validated text as UTF-8.
#[derive(Debug)]
pub enum StyleSheetSource {
    Preparsed(Arc<PreparsedStyleSheet>),
    Text(String),
}

/// A loaded source. `Entry` carries JavaScript for a main entry, imported
/// module or worker script, including its final response URL. The completion
/// routes it to the runtime that requested it.
#[derive(Debug)]
pub enum LoadedSource {
    StyleSheet(StyleSheetSource),
    Entry { source: String, url: String },
}

/// The concrete, transferable right to answer one source request.
///
/// This is neither a closure nor a trait object, and it names no destination:
/// it holds one end of the one-shot channel that was minted with the request,
/// and whoever awaits the other end is where the source goes. That is what
/// lets a worker's script skip `bobcat-main` entirely while a stylesheet's
/// reaches the task that asked for it, with one type and no routing.
///
/// It cannot be cloned; consuming it permits at most one result. An unanswered
/// drop reports failure, so a lost worker cannot leave startup waiting forever.
#[must_use = "complete the source request or retain it until the load finishes"]
pub struct SourceCompletion {
    answer: Option<tokio::sync::oneshot::Sender<Result<LoadedSource, crate::LynxViewError>>>,
    cancel: crate::link::ViewCancel,
}

impl std::fmt::Debug for SourceCompletion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SourceCompletion")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl SourceCompletion {
    /// One request's two ends, for a caller that will await the answer itself.
    pub(crate) fn new(
        cancel: crate::link::ViewCancel,
    ) -> (
        Self,
        tokio::sync::oneshot::Receiver<Result<LoadedSource, crate::LynxViewError>>,
    ) {
        let (answer, receiver) = tokio::sync::oneshot::channel();
        (Self::over(answer, cancel), receiver)
    }

    /// The right to answer a request whose receiving end has already been
    /// handed to whoever is waiting for it.
    pub(crate) fn over(
        answer: tokio::sync::oneshot::Sender<Result<LoadedSource, crate::LynxViewError>>,
        cancel: crate::link::ViewCancel,
    ) -> Self {
        Self {
            answer: Some(answer),
            cancel,
        }
    }

    /// Whether the view has ended, or nobody is waiting for this source any
    /// more. Cancellation is cooperative: an IO operation already running may
    /// finish, but its result is discarded.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
            || self
                .answer
                .as_ref()
                .is_none_or(tokio::sync::oneshot::Sender::is_closed)
    }

    /// Sends the result once. A result for a cancelled view is discarded.
    pub fn complete(mut self, source: Result<LoadedSource, crate::LynxViewError>) {
        self.send(source);
    }

    fn send(&mut self, source: Result<LoadedSource, crate::LynxViewError>) {
        if let Some(answer) = self.answer.take()
            && !self.cancel.is_cancelled()
        {
            let _ = answer.send(source);
        }
    }
}

impl Drop for SourceCompletion {
    fn drop(&mut self) {
        if !self.is_cancelled() {
            self.send(Err(unanswered_source().into()));
        }
    }
}

/// What a source that nobody answered failed with — reported by a dropped
/// completion, and by the awaiting side when the completion never reached a
/// drop of its own.
pub(crate) fn unanswered_source() -> ResourceError {
    ResourceError {
        request_id: None,
        kind: ResourceErrorKind::Unavailable,
        phase: ResourceErrorPhase::ReadBody,
        locator: None,
        status: None,
        message: "the fetcher dropped a source request without completing it".into(),
        retry: RetryAdvice::Never,
    }
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

/// A test host with no sources or readable images.
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
    fn request_source(&self, _request: SourceRequest, _completion: SourceCompletion) {}

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

#[cfg(test)]
mod completion_tests {
    use tokio::sync::oneshot::error::TryRecvError;

    use super::*;
    use crate::link::ViewCancel;

    fn completion() -> (
        SourceCompletion,
        tokio::sync::oneshot::Receiver<Result<LoadedSource, crate::LynxViewError>>,
        ViewCancel,
    ) {
        let cancel = ViewCancel::default();
        let (completion, answer) = SourceCompletion::new(cancel.clone());
        (completion, answer, cancel)
    }

    fn source() -> LoadedSource {
        LoadedSource::Entry {
            source: String::new(),
            url: "app:///main.js".into(),
        }
    }

    #[test]
    fn a_completion_sends_exactly_one_result_from_wherever_the_load_finished() {
        let (completion, mut answer, _) = completion();
        std::thread::spawn(move || completion.complete(Ok(source())))
            .join()
            .unwrap();
        assert!(matches!(answer.try_recv(), Ok(Ok(_))));
        assert!(matches!(answer.try_recv(), Err(TryRecvError::Closed)));
    }

    #[test]
    fn an_unanswered_drop_reports_failure() {
        let (completion, mut answer, _) = completion();
        drop(completion);
        assert!(matches!(
            answer.try_recv(),
            Ok(Err(crate::LynxViewError::Resource(ResourceError {
                kind: ResourceErrorKind::Unavailable,
                ..
            })))
        ));
        assert!(matches!(answer.try_recv(), Err(TryRecvError::Closed)));
    }

    #[test]
    fn cancellation_discards_both_late_results_and_unanswered_drops() {
        for answer_it in [false, true] {
            let (completion, mut answer, cancel) = completion();
            cancel.cancel();
            assert!(completion.is_cancelled());
            if answer_it {
                completion.complete(Ok(source()));
            } else {
                drop(completion);
            }
            assert!(matches!(answer.try_recv(), Err(TryRecvError::Closed)));
        }
    }

    #[test]
    fn a_destination_that_stopped_waiting_cancels_the_source() {
        let (completion, answer, _) = completion();
        drop(answer);
        assert!(completion.is_cancelled());
    }
}
