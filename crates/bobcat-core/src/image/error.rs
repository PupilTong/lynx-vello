//! The single error type every fallible entry point in this module returns.

use std::sync::Arc;

use thiserror::Error;

use crate::image::format::ImageFormat;
use crate::resource::ResourceError;

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum ImageError {
    #[error("resource acquisition failed: {0}")]
    Resource(#[from] ResourceError),

    #[error("resource fetcher supports no usable transport")]
    NoTransport,

    #[error("unrecognised image container")]
    UnknownFormat,

    #[error("{format} is not supported by the injected decoder")]
    Unsupported { format: ImageFormat },

    #[error("truncated {format} data ({len} bytes)")]
    Truncated { format: ImageFormat, len: usize },

    #[error("{format} decode failed: {message}")]
    Decode {
        format: ImageFormat,
        message: Arc<str>,
    },

    #[error("image is {width}x{height}, past the decode limit ({limit})")]
    TooLarge {
        width: u32,
        height: u32,
        limit: Arc<str>,
    },

    #[error("encoded image exceeds the {limit}-byte budget")]
    EncodedTooLarge { limit: u64 },

    #[error("malformed data: URL: {0}")]
    MalformedDataUrl(Arc<str>),

    #[error("{context}: {message}")]
    Transport {
        context: &'static str,
        message: Arc<str>,
    },

    #[error("image load cancelled")]
    Cancelled,
}

impl ImageError {
    /// A [`Self::Decode`] with `message`.
    pub fn decode(format: ImageFormat, message: impl Into<Arc<str>>) -> Self {
        Self::Decode {
            format,
            message: message.into(),
        }
    }

    /// A [`Self::TooLarge`] naming the breached limit.
    pub fn too_large(width: u32, height: u32, limit: impl Into<Arc<str>>) -> Self {
        Self::TooLarge {
            width,
            height,
            limit: limit.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn transport(context: &'static str, error: &std::io::Error) -> Self {
        Self::Transport {
            context,
            message: error.to_string().into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn decode_join(error: &tokio::task::JoinError) -> Self {
        if error.is_cancelled() {
            return Self::Cancelled;
        }
        Self::Transport {
            context: "decode task",
            message: error.to_string().into(),
        }
    }
}
