use std::str::Utf8Error;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConvertError {
    #[error(
        "unexpected end of native bundle at byte {offset}: needed {needed} bytes, {remaining} remain"
    )]
    UnexpectedEof {
        offset: usize,
        needed: usize,
        remaining: usize,
    },

    #[error("native bundle declares {declared} bytes but input contains {actual}")]
    SizeMismatch { declared: u32, actual: usize },

    #[error("not a supported Lynx native bundle: magic 0x{0:08x}")]
    BadNativeMagic(u32),

    #[error("unsupported native bundle target SDK version {0}; version 2.8 or newer is required")]
    UnsupportedVersion(String),

    #[error("invalid native bundle at byte {offset}: {reason}")]
    InvalidNative { offset: usize, reason: String },

    #[error("invalid UTF-8 in {context} at byte {offset}: {source}")]
    InvalidUtf8 {
        context: &'static str,
        offset: usize,
        #[source]
        source: Utf8Error,
    },

    #[error("invalid JSON in native {section} section: {source}")]
    InvalidJson {
        section: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to serialize web {section} JSON: {source}")]
    OutputJson {
        section: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "cannot convert code-cache bundle: native section {section:?} contains bytecode without recoverable JavaScript source"
    )]
    CodeCacheBundle { section: String },

    #[error("unsupported native bundle: {0}")]
    UnsupportedBundle(String),

    #[error("unsupported native CSS encoding: {0}")]
    UnsupportedCss(String),

    #[error("failed to serialize web StyleInfo: {0}")]
    StyleInfo(String),

    #[error("web section {section} is too large: {length} bytes")]
    OutputTooLarge {
        section: &'static str,
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
