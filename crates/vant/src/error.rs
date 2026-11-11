//! The resource error vocabulary: the protocol's error value every
//! operation reports, and [`Failure`], which builds it with the fields every
//! failure fills the same way.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

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

/// A failure before it has a resource to belong to: everything but the
/// locator, which the caller that has it adds.
#[derive(Clone, Debug)]
pub struct Failure {
    pub kind: ResourceErrorKind,
    pub phase: ResourceErrorPhase,
    pub message: String,
    pub retry: RetryAdvice,
}

impl Failure {
    #[must_use]
    pub fn new(
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

    #[must_use]
    pub fn with_retry(mut self, retry: RetryAdvice) -> Self {
        self.retry = retry;
        self
    }

    /// The protocol error for this failure, naming `locator`.
    #[must_use]
    pub fn into_error(self, locator: Option<Arc<str>>) -> ResourceError {
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
