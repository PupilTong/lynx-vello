//! The single error type every fallible entry point in this module returns.

use std::sync::Arc;

use thiserror::Error;

use crate::image::format::ImageFormat;
use crate::resource::ResourceError;

/// Everything that can go wrong between a specifier and decoded pixels.
///
/// Deliberately a typed enum rather than Lynx's `error_code` /
/// `lynx_categorized_code` integers: the network and user buckets are already
/// better typed one layer down as [`ResourceError`], only the picture-source
/// bucket is this module's to classify, and the integers' sole consumer is an
/// `error` event detail that needs an event model this project does not have
/// yet.
#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum ImageError {
    /// The host's [`ResourceFetcher`](crate::resource::ResourceFetcher)
    /// failed to resolve, open or read the bytes.
    #[error("resource acquisition failed: {0}")]
    Resource(#[from] ResourceError),

    /// The fetcher advertises none of the transports this module can read bytes
    /// through. Checked once when the loader is built, not per image, because
    /// only `resolve_locator` and `cancel_request` are mandatory in the
    /// protocol.
    #[error("resource fetcher supports no usable transport")]
    NoTransport,

    /// The leading bytes match no container this module identifies.
    #[error("unrecognised image container")]
    UnknownFormat,

    /// A container this module identifies, but the injected decoder does not
    /// claim. Distinct from [`Self::UnknownFormat`] because the two want
    /// different diagnostics: a HEIC on a Linux host is a recognised format the
    /// running decoder cannot serve, not line noise.
    #[error("{format} is not supported by the injected decoder")]
    Unsupported { format: ImageFormat },

    /// The container's own framing says bytes are missing. Checked before any
    /// backend runs, because the backends disagree about truncation: `ImageIO`
    /// hands back a fully transparent full-size image and reports the source
    /// complete, where the software decoders error.
    #[error("truncated {format} data ({len} bytes)")]
    Truncated { format: ImageFormat, len: usize },

    /// The decoder rejected well-framed bytes.
    #[error("{format} decode failed: {message}")]
    Decode {
        format: ImageFormat,
        message: Arc<str>,
    },

    /// Rejected before allocation by [`DecodeRequest`](crate::image::DecodeRequest)'s
    /// caps. A hard rejection rather than a clamp: vello packs every scene
    /// image into one shared atlas, and an image it cannot allocate is silently
    /// not rendered.
    #[error("image is {width}x{height}, past the decode limit ({limit})")]
    TooLarge {
        width: u32,
        height: u32,
        limit: Arc<str>,
    },

    /// Encoded bytes past the loader's `max_encoded_bytes` budget, refused
    /// before the whole body was buffered.
    #[error("encoded image exceeds the {limit}-byte budget")]
    EncodedTooLarge { limit: u64 },

    /// A `data:` URL whose payload could not be parsed.
    #[error("malformed data: URL: {0}")]
    MalformedDataUrl(Arc<str>),

    /// I/O below the protocol's own error type: draining a
    /// [`ResourceStream`](crate::resource::ResourceStream), or reading a
    /// [`ResourcePath`](crate::resource::ResourcePath) off disk. The
    /// fetcher succeeded; moving the bytes afterwards did not.
    #[error("{context}: {message}")]
    Transport {
        context: &'static str,
        message: Arc<str>,
    },

    /// The caller's [`CancellationToken`](tokio_util::sync::CancellationToken)
    /// fired. A decode already inside a blocking task still drains; its result
    /// is discarded rather than published.
    #[error("image load cancelled")]
    Cancelled,
}

impl ImageError {
    /// A [`Self::Decode`] with `message`. Public because decoder
    /// implementations live outside this crate (`image-decoders`, or an
    /// embedder's own) and every one of them needs to construct it.
    pub fn decode(format: ImageFormat, message: impl Into<Arc<str>>) -> Self {
        Self::Decode {
            format,
            message: message.into(),
        }
    }

    /// A [`Self::TooLarge`] naming the breached limit. Public for the same
    /// reason as [`Self::decode`].
    pub fn too_large(width: u32, height: u32, limit: impl Into<Arc<str>>) -> Self {
        Self::TooLarge {
            width,
            height,
            limit: limit.into(),
        }
    }

    pub(crate) fn transport(context: &'static str, error: &std::io::Error) -> Self {
        Self::Transport {
            context,
            message: error.to_string().into(),
        }
    }

    /// A blocking decode task that panicked or was aborted. Cancellation is
    /// reported as [`Self::Cancelled`] rather than folded in here, so a genuine
    /// panic in a decoder stays visible instead of looking like a user action.
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
