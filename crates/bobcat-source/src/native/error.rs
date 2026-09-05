use std::str::Utf8Error;

/// Errors produced while converting a native Lynx external bundle.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConvertError {
    /// The input ended in the middle of a native-bundle field.
    #[error(
        "unexpected end of native bundle at byte {offset}: needed {needed} bytes, {remaining} remain"
    )]
    UnexpectedEof {
        /// Absolute byte offset of the failed read.
        offset: usize,
        /// Number of requested bytes.
        needed: usize,
        /// Number of available bytes.
        remaining: usize,
    },

    /// The declared native-bundle size does not match the input length.
    #[error("native bundle declares {declared} bytes but input contains {actual}")]
    SizeMismatch {
        /// Size stored in the native header.
        declared: u32,
        /// Actual input length.
        actual: usize,
    },

    /// The native magic word is not supported.
    #[error("not a supported Lynx native bundle: magic 0x{0:08x}")]
    BadNativeMagic(u32),

    /// The native target version is too old for routed external sections.
    #[error("unsupported native bundle target SDK version {0}; version 2.8 or newer is required")]
    UnsupportedVersion(String),

    /// A structural native-bundle invariant was violated.
    #[error("invalid native bundle at byte {offset}: {reason}")]
    InvalidNative {
        /// Absolute byte offset associated with the problem.
        offset: usize,
        /// Human-readable reason.
        reason: String,
    },

    /// A native string was not valid UTF-8.
    #[error("invalid UTF-8 in {context} at byte {offset}: {source}")]
    InvalidUtf8 {
        /// Field or section being decoded.
        context: &'static str,
        /// Absolute byte offset of the string data.
        offset: usize,
        /// UTF-8 validation failure.
        #[source]
        source: Utf8Error,
    },

    /// A native JSON configuration section was invalid.
    #[error("invalid JSON in native {section} section: {source}")]
    InvalidJson {
        /// Native section name.
        section: &'static str,
        /// JSON parser failure.
        #[source]
        source: serde_json::Error,
    },

    /// A generated web JSON section could not be serialized.
    #[error("failed to serialize web {section} JSON: {source}")]
    OutputJson {
        /// Web section name.
        section: &'static str,
        /// JSON serializer failure.
        #[source]
        source: serde_json::Error,
    },

    /// The bundle contains `QuickJS` bytecode/code cache rather than source.
    #[error(
        "cannot convert code-cache bundle: native section {section:?} contains bytecode without recoverable JavaScript source"
    )]
    CodeCacheBundle {
        /// Section or custom-section name containing bytecode.
        section: String,
    },

    /// The bundle uses a native feature outside the source-external format.
    #[error("unsupported native bundle: {0}")]
    UnsupportedBundle(String),

    /// The native CSS fragment could not be represented as web `StyleInfo`.
    #[error("unsupported native CSS encoding: {0}")]
    UnsupportedCss(String),

    /// Serializing the web `StyleInfo` archive failed.
    #[error("failed to serialize web StyleInfo: {0}")]
    StyleInfo(String),

    /// A web section exceeded its 32-bit wire length.
    #[error("web section {section} is too large: {length} bytes")]
    OutputTooLarge {
        /// Web section name.
        section: &'static str,
        /// Attempted payload size.
        length: usize,
    },
}

impl ConvertError {
    pub(crate) fn invalid(offset: usize, reason: impl Into<String>) -> Self {
        Self::InvalidNative {
            offset,
            reason: reason.into(),
        }
    }
}
