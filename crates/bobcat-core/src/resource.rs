//! Host-injected resource acquisition contracts for Bobcat.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::style::PreparsedStyleSheet;

/// The host's whole resource system: sources, stylesheets and images.
///
/// Owned by the [`LynxView`](crate::LynxView), on the thread that constructed
/// it, and never reachable from `bobcat-main` — every resource the document
/// needs is asked for by message. It is therefore free to be neither `Send`
/// nor `Sync` and to hold `Rc`, `RefCell` or browser objects directly.
///
/// The view services it in [`LynxView::pump`](crate::LynxView::pump) and
/// nowhere else, past the one call that builds it: that call hands it the
/// view's startup sources as well, so their IO overlaps everything the
/// embedder does before its first turn. A painter observing that view reads
/// pixels out of it through [`FrameImages`](dom::FrameImages) while it
/// composes, and asks it for nothing.
///
/// Source requests are non-blocking: the fetcher resolves the URL, loads and
/// validates UTF-8 (or returns a pre-parsed sheet), then consumes the concrete
/// [`SourceCompletion`] to answer whoever asked, without naming them. It owns
/// any executor its IO needs. Images are reported through
/// [`ImageReports`](dom::ImageReports), then read during composition.
///
/// Every protocol method is synchronous: it starts work and returns. Core therefore holds no
/// resource future and polls none, and nothing here names a host's transport, caches or
/// codecs: whatever surface those have belongs to the host's own crate.
pub trait ResourceFetcher: dom::FrameImages {
    /// Begins one source load without blocking its caller.
    ///
    /// **A view's startup sources are requested before it runs at all**:
    /// every author stylesheet in cascade order and then the entry are handed
    /// over together inside
    /// [`LynxGroup::create_lynx_view`](crate::LynxGroup::create_lynx_view),
    /// on the embedder's own thread, before it returns — this fetcher is
    /// built earlier in that same call, out of the builder the embedder
    /// passed, so a fetcher must be able to take them there. Order of
    /// *completion* is this method's own business: the view reads the answers
    /// in cascade order and then the entry whatever order they arrive in.
    /// Every later request — an import, an adopted stylesheet, a worker
    /// script, a font, a plain fetch — is handed over in a
    /// [`LynxView::pump`](crate::LynxView::pump) turn instead.
    ///
    /// Consume `completion` with the result, or retain it until the load finishes.
    /// Dropping it unanswered reports a failure unless the view has ended.
    /// Check [`SourceCompletion::is_cancelled`] before starting queued work and
    /// after IO — it answers whether the view was cancelled by the embedder's
    /// release or a fatal event, or ended on its own — since a cancelled load
    /// no longer needs to decode or deliver a result.
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion);

    /// Hints that a later source request may use this URL. The fetcher owns
    /// preloading, in-flight sharing and cache lifetime; no response is required.
    /// The default ignores the hint, so preloading is never required for correctness.
    fn preload_source(&self, _request: SourceRequest) {}

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

    /// The host's own moment in every [`LynxView::pump`](crate::LynxView::pump),
    /// on this thread, before the turn reads the reports queued so far.
    ///
    /// A host whose loads finish somewhere else — a decode thread, a
    /// browser worker — forwards each completion into its
    /// [`ImageReports`](dom::ImageReports) here, so a load that completed
    /// between turns is reported in the next one whether or not that turn
    /// requested or resolved anything. Asking for that turn is still the
    /// host's, through the wakeup it gave the view. A host that reports
    /// inline has nothing to do, and the default does nothing.
    fn service_images(&self) {}

    /// A thread-safe answer to whether a [`SourceRequest::Fetch`] of `url`
    /// has already completed for this view, asked from a realm's own thread
    /// with the URL string the realm holds — which the probe resolves the way
    /// the fetcher resolves a request.
    ///
    /// A fetch it answers `true` for is **not made again**: the realm's
    /// `fetchResource` settles at once, which is what a cached fetch looks
    /// like from JavaScript. That is the whole reason it exists — a Promise
    /// resolved in a later job is not what a repeat fetch is to a card, and
    /// both native (`TemplateAssembler::FindTemplateBundle`) and web-core (a
    /// promise cache) settle one inside the same task.
    ///
    /// It is called from `bobcat-main` and from `bobcat-workers`, never from
    /// the view's own thread, which is why it is a `Send + Sync` function
    /// rather than a method: nothing else on this trait leaves the embedder's
    /// thread. It must not block and must not start a load.
    ///
    /// The default is no probe: every `fetchResource` is a request, and none
    /// settles synchronously.
    fn fetch_probe(&self) -> Option<FetchProbe> {
        None
    }
}

/// What a fetcher answers [`ResourceFetcher::fetch_probe`] with: "has this
/// view already fetched `url`?", callable from either engine thread.
pub type FetchProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// A shared handle serves whatever it points at.
///
/// The view owns its resource system; an embedder whose registry outlives the
/// view hands in an [`Rc`] of it instead. This is what joins the two without a
/// per-embedder forwarding wrapper. `Rc` rather than `Arc` because nothing on
/// this path crosses a thread — an atomic count here would be paid on every
/// clone and never used.
///
/// It is the right handle for a registry that answers reads and starts loads,
/// and the wrong one for a registry that *reports* — an [`ImageReports`](dom::ImageReports)
/// belongs to one view, and a handle shared across views has nowhere to put
/// more than one. A host that loads images asynchronously returns a per-view
/// value from the builder [`create_lynx_view`](crate::LynxGroup::create_lynx_view) takes,
/// holding this handle plus that view's reports.
impl<T: ResourceFetcher + ?Sized> ResourceFetcher for Rc<T> {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        (**self).request_source(request, completion);
    }

    fn preload_source(&self, request: SourceRequest) {
        (**self).preload_source(request);
    }

    fn request_image(&self, source: &str) {
        (**self).request_image(source);
    }

    fn service_images(&self) {
        (**self).service_images();
    }

    fn fetch_probe(&self) -> Option<FetchProbe> {
        (**self).fetch_probe()
    }
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
    /// Fetch `url` and keep it, the way an image source is fetched,
    /// answering only that the fetch is over.
    ///
    /// What the fetcher makes of the bytes is its own — the reference
    /// fetcher, given a container installer, registers a Lynx container's
    /// sections beside it — and nothing about them comes back. Resolution is
    /// the fetcher's, as for every request.
    ///
    /// Complete with [`LoadedSource::Fetched`].
    Fetch {
        url: String,
    },
    /// One `src: url(...)` of an `@font-face` rule, already resolved against
    /// the document's base URL. Complete with [`LoadedSource::Font`]; the
    /// bytes are registered under the rule's declared family. A fetcher that
    /// serves no fonts drops the completion, which fails this source and lets
    /// the document try the rule's next one.
    Font {
        url: String,
    },
}

/// A stylesheet ready to mount. The fetcher has already validated text as UTF-8.
#[derive(Clone, Debug)]
pub enum StyleSheetSource {
    Preparsed(Arc<PreparsedStyleSheet>),
    Text(String),
}

/// A loaded source. `Entry` carries JavaScript for a main entry, imported
/// module or worker script, including its final response URL. The completion
/// routes it to the runtime that requested it.
#[derive(Clone, Debug)]
pub enum LoadedSource {
    StyleSheet(StyleSheetSource),
    Entry {
        source: String,
        url: String,
    },
    /// One `@font-face` source's bytes, unvalidated: a font file is binary,
    /// so nothing here decodes or transcodes it. The font backend is what
    /// rejects bytes that are not a face.
    Font(dom::FontBlob),
    /// A [`SourceRequest::Fetch`] that is over. It carries nothing: the
    /// bytes stayed with the fetcher, and whatever it made of them is
    /// reachable only as the source requests it now answers.
    Fetched,
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
    token: tokio_util::sync::CancellationToken,
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
        token: tokio_util::sync::CancellationToken,
    ) -> (
        Self,
        tokio::sync::oneshot::Receiver<Result<LoadedSource, crate::LynxViewError>>,
    ) {
        let (answer, receiver) = tokio::sync::oneshot::channel();
        (Self::over(answer, token), receiver)
    }

    /// The right to answer a request whose receiving end has already been
    /// handed to whoever is waiting for it.
    pub(crate) fn over(
        answer: tokio::sync::oneshot::Sender<Result<LoadedSource, crate::LynxViewError>>,
        token: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            answer: Some(answer),
            token,
        }
    }

    /// Whether the requesting realm has ended, or nobody is waiting for this
    /// source any more.
    ///
    /// Completions carry their requesting realm's cancellation token: the
    /// view's for MTS, or the Worker's independent token for its entry/imports.
    /// Reading it takes a mutex, so it is
    /// asked once per decision rather than per byte. Cancellation is
    /// cooperative: an IO operation already running may finish, but its result
    /// is discarded.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
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
            && !self.token.is_cancelled()
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
        kind: ResourceErrorKind::Unavailable,
        phase: ResourceErrorPhase::ReadBody,
        locator: None,
        message: "the fetcher dropped a source request without completing it".into(),
        retry: RetryAdvice::Never,
    }
}

/// Stable resource failure details shared by every operation.
#[derive(Clone, Debug, Error)]
#[error("{kind:?} during {phase:?}: {message}")]
pub struct ResourceError {
    pub kind: ResourceErrorKind,
    pub phase: ResourceErrorPhase,
    pub locator: Option<Arc<str>>,
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
mod completion_tests {
    use tokio::sync::oneshot::error::TryRecvError;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn completion() -> (
        SourceCompletion,
        tokio::sync::oneshot::Receiver<Result<LoadedSource, crate::LynxViewError>>,
        CancellationToken,
    ) {
        let token = CancellationToken::new();
        let (completion, answer) = SourceCompletion::new(token.clone());
        (completion, answer, token)
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
            let (completion, mut answer, token) = completion();
            token.cancel();
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
