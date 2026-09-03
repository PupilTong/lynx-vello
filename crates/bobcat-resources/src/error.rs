//! Building the protocol's error value with the fields every failure here
//! fills the same way.

use std::sync::Arc;

use bobcat_core::resource::{
    RequestId, ResourceError, ResourceErrorKind, ResourceErrorPhase, RetryAdvice,
};
use http::StatusCode;

/// A failure before it has a request to belong to: everything but the
/// request id and locator, which the caller that has them adds.
#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub kind: ResourceErrorKind,
    pub phase: ResourceErrorPhase,
    pub status: Option<StatusCode>,
    pub message: String,
    pub retry: RetryAdvice,
}

impl Failure {
    pub(crate) fn new(
        kind: ResourceErrorKind,
        phase: ResourceErrorPhase,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            phase,
            status: None,
            message: message.into(),
            retry: RetryAdvice::Never,
        }
    }

    pub(crate) fn with_status(mut self, status: StatusCode) -> Self {
        self.status = Some(status);
        self
    }

    pub(crate) fn with_retry(mut self, retry: RetryAdvice) -> Self {
        self.retry = retry;
        self
    }

    /// The protocol error for `request_id` naming `locator`.
    pub(crate) fn into_error(
        self,
        request_id: Option<RequestId>,
        locator: Option<Arc<str>>,
    ) -> ResourceError {
        ResourceError {
            request_id,
            kind: self.kind,
            phase: self.phase,
            locator,
            status: self.status,
            message: Arc::from(self.message),
            retry: self.retry,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:?} during {:?}: {}",
            self.kind, self.phase, self.message
        )
    }
}
