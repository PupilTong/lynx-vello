//! Host-injected resource acquisition contracts for Bobcat.

use std::rc::Rc;
use std::sync::Arc;

pub use vant::error::{ResourceError, ResourceErrorKind, ResourceErrorPhase, RetryAdvice};

use crate::style::PreparsedStyleSheet;

pub trait ResourceFetcher: terminus::link::FrameImages {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion);

    fn preload_source(&self, _request: SourceRequest) {}

    fn request_image(&self, _source: &str) {}

    fn request_document(
        &self,
        _source: &str,
        _bytes: bytes::Bytes,
        _kind: terminus::link::DocumentKind,
    ) {
    }

    fn request_playback(&self, _request: terminus::link::PlaybackRequest) {}

    fn service_images(&self) {}

    fn fetch_probe(&self) -> Option<FetchProbe> {
        None
    }

    fn style_sheet_probe(&self) -> Option<StyleSheetProbe> {
        None
    }
}

pub type FetchProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub type StyleSheetProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;

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

    fn request_document(
        &self,
        source: &str,
        bytes: bytes::Bytes,
        kind: terminus::link::DocumentKind,
    ) {
        (**self).request_document(source, bytes, kind);
    }

    fn request_playback(&self, request: terminus::link::PlaybackRequest) {
        (**self).request_playback(request);
    }

    fn service_images(&self) {
        (**self).service_images();
    }

    fn fetch_probe(&self) -> Option<FetchProbe> {
        (**self).fetch_probe()
    }

    fn style_sheet_probe(&self) -> Option<StyleSheetProbe> {
        (**self).style_sheet_probe()
    }
}

#[derive(Debug)]
pub enum SourceRequest {
    StyleSheet(String),
    Module(String),
    Fetch { url: String, timed: bool },
    Font { url: String },
}

#[derive(Clone, Debug)]
pub enum StyleSheetSource {
    Preparsed(Arc<PreparsedStyleSheet>),
    Text(String),
}

#[derive(Clone, Debug)]
pub enum LoadedSource {
    StyleSheet(StyleSheetSource),
    Module { source: String, url: String },
    Font(terminus::link::FontBlob),
    Fetched(FetchOutcome),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchOutcome {
    pub size_bytes: Option<u64>,
    pub received: Option<crate::performance::Instant>,
    pub installed: Option<std::ops::Range<crate::performance::Instant>>,
}

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
    pub(crate) fn new(
        token: tokio_util::sync::CancellationToken,
    ) -> (
        Self,
        tokio::sync::oneshot::Receiver<Result<LoadedSource, crate::LynxViewError>>,
    ) {
        let (answer, receiver) = tokio::sync::oneshot::channel();
        (Self::over(answer, token), receiver)
    }

    pub(crate) fn over(
        answer: tokio::sync::oneshot::Sender<Result<LoadedSource, crate::LynxViewError>>,
        token: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            answer: Some(answer),
            token,
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
            || self
                .answer
                .as_ref()
                .is_none_or(tokio::sync::oneshot::Sender::is_closed)
    }

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

pub(crate) fn unanswered_source() -> ResourceError {
    ResourceError {
        kind: ResourceErrorKind::Unavailable,
        phase: ResourceErrorPhase::ReadBody,
        locator: None,
        message: "the fetcher dropped a source request without completing it".into(),
        retry: RetryAdvice::Never,
    }
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
        LoadedSource::Module {
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
