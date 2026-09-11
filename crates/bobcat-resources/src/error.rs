//! Building the protocol's error value with the fields every failure here
//! fills the same way.

use std::sync::Arc;

use bobcat_core::resource::{ResourceError, ResourceErrorKind, ResourceErrorPhase, RetryAdvice};

/// A failure before it has a resource to belong to: everything but the
/// locator, which the caller that has it adds.
#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub kind: ResourceErrorKind,
    pub phase: ResourceErrorPhase,
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
            message: message.into(),
            retry: RetryAdvice::Never,
        }
    }

    pub(crate) fn with_retry(mut self, retry: RetryAdvice) -> Self {
        self.retry = retry;
        self
    }

    /// The protocol error for this failure, naming `locator`.
    pub(crate) fn into_error(self, locator: Option<Arc<str>>) -> ResourceError {
        ResourceError {
            kind: self.kind,
            phase: self.phase,
            locator,
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
