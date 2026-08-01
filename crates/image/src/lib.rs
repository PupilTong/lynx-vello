//! `image` — encoded bytes in, decoded pixels out.
//!
//! This crate owns the replaced-content pipeline below the DOM: it identifies a
//! container from its leading bytes ([`sniff`]), probes intrinsic dimensions
//! without decoding ([`Decoder::probe`]), decodes to RGBA8 through the best
//! backend the running machine offers ([`BackendRegistry`]), and — through
//! [`ImageLoader`] — drives that from a host [`ResourceFetcher`] with byte
//! budgets, cancellation, and bounded caches for both the pixels and the much
//! cheaper natural sizes.
//!
//! It deliberately depends on **neither `dom` nor `pulsar`**. It returns an
//! [`ImageHeader`] and a [`DecodedImage`]; installing the natural size on a node
//! and the pixels in a paint-side store is the caller's job, and ordering that
//! against the style flush is the caller's problem. That boundary is what keeps
//! the decoder off the DOM and what lets the Windows and Android backends be
//! type-checked from any host with nothing but `cargo check --target`.
//!
//! # Backends and what "hardware decoding" actually means here
//!
//! One pure-Rust backend is always compiled in and always claims all three
//! formats. At most one platform backend joins it, chosen at runtime because the
//! question is genuinely a runtime one — `ImageIO` gained WebP in macOS 11 / iOS
//! 14, WIC's WebP codec is a Store extension rather than an inbox component, and
//! `AImageDecoder` exists only from Android API 30.
//!
//! [`Acceleration`] reports codec **provenance**, not silicon. No still-image
//! API on any of the three platforms exposes an acceleration query, and none of
//! them reaches a dedicated decode ASIC, so [`Acceleration::DedicatedHardware`]
//! is reserved and never reported. Claiming otherwise would be the easy lie; the
//! ladder we can actually observe is the honest answer.
//!
//! # Deliberate v1 limits (the compat bar is behavioral, not pixel-perfect — see AGENTS.md)
//!
//! 1. PNG, JPEG and WebP only, and static only. An animated WebP or APNG decodes to frame 0 with
//!    [`ImageHeader::animated`] set and the rest discarded; GIF, AVIF and SVG do not decode at all.
//!    No loop count or frame delay is retained, so Lynx's
//!    `loop-count`/`startplay`/`currentloopcomplete` surface has nothing to read even once an
//!    element layer exists.
//! 2. No backend reaches a decode ASIC, so [`Acceleration`] is a provenance ladder rather than a
//!    claim about silicon, and its top rung is unreported.
//! 3. On Apple, PNG deliberately routes to the software backend: `ImageIO` delegates PNG to its own
//!    bundled `libpng`, which measured slower than the `png` crate this workspace already links.
//!    Routing is allowed to disagree with the provenance ladder when provenance and speed diverge.
//! 4. Only the platform backends downsample *during* decode. The software backend decodes at full
//!    resolution and resamples, so it pays peak full-size memory for a small retained bitmap — a
//!    memory profile difference, not a behavioral one.
//! 5. [`DecodeRequest::max_dimension`] defaults to 8192 and is a hard rejection rather than a
//!    clamp, because vello packs every scene image into one shared atlas capped at that size and an
//!    image it cannot allocate is silently not rendered.
//! 6. Colour management is not performed. Decoded bytes are the file's own sRGB-encoded values with
//!    no ICC or CICP conversion and no gamma conversion, because vello's atlas is `Rgba8Unorm`
//!    rather than `Rgba8UnormSrgb`. A wide-gamut or tagged image renders as if it were sRGB.
//! 7. Alpha encoding is carried, not normalised — see [`AlphaType`]. Byte-identical output across
//!    backends is not a goal; identical composited output is.
//! 8. EXIF orientation is honoured for JPEG only. PNG's `eXIf` chunk and WebP's `EXIF` chunk are
//!    not read.
//! 9. One node owns exactly one in-flight request. Lynx `<image>`'s concurrent src/placeholder
//!    race, its last-wins arbitration and its "src success permanently suppresses placeholder" lock
//!    are element policy that belongs above this crate; all this crate owes upward is per-request
//!    cancellation.
//! 10. No retry and no backoff. One failed fetch or decode is one terminal error, matching native
//!     Lynx, where no retry path exists anywhere; the resource protocol's `RetryAdvice` already
//!     defaults to `Never`.
//! 11. Decode cancellation is cooperative on the async side only. A decode already inside a
//!     blocking task cannot be aborted — the platform backends are single one-shot C calls — so a
//!     cancelled load returns promptly while the decode drains and its result is discarded. Decode
//!     concurrency is bounded so drained work cannot starve the pool.
//! 12. Errors are a typed enum, not Lynx's `error_code`/`lynx_categorized_code` integers. See
//!     [`ImageError`].
//! 13. `cap-insets` (9-slice), `blur-radius`, `region-to-decode`, `tint-color` and the iOS fade-in
//!     are absent — all Lynx `<image>` extensions with no W3C `<img>` counterpart.
//! 14. One image pixel is one CSS pixel. There are no density descriptors (`srcset`/`sizes` are not
//!     implemented), so nothing here needs a scale factor; when density lands, the conversion
//!     belongs on [`ImageHeader`] rather than at each call site.
//! 15. The WIC and `AImageDecoder` backends are type-checked but unexecuted — no Windows or Android
//!     runner exists for this workspace yet. Both degrade to the software backend rather than
//!     failing, because the capability probe has to succeed before either is used.
//!
//! [`ResourceFetcher`]: bobcat_core::resource::ResourceFetcher

// The coverage run compiles with `--cfg coverage_nightly` and the test modules
// opt out via `#[coverage(off)]`, which needs this experimental feature (same
// pattern as every other workspace crate).
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod backend;
mod cache;
mod data_url;
mod decode;
mod error;
mod format;
mod loader;
mod orientation;
mod pixels;
mod registry;

pub use backend::software::SoftwareDecoder;
pub use cache::{CacheKey, DecodeCache, HeaderCache};
pub use decode::{DecodeRequest, DecodeResponse, Decoder, ImageHeader, PixelSize};
pub use error::ImageError;
pub use format::{ImageFormat, is_complete, sniff};
pub use loader::{ImageLoader, ImagePrefetchTarget, LoaderConfig};
pub use pixels::{AlphaType, DecodedImage};
pub use registry::{Acceleration, BackendRegistry, Capabilities};

/// Identifies, validates and decodes one in-memory image with the best backend
/// this machine offers.
///
/// The one-shot path for a caller that already has the bytes — the `data:`
/// branch, a test, a fixture. Anything fetching over a transport wants
/// [`ImageLoader`], which adds caching, byte budgets and cancellation.
///
/// # Errors
///
/// [`ImageError::UnknownFormat`] when the leading bytes match no supported
/// container, [`ImageError::Truncated`] when the container's own framing says
/// bytes are missing, and [`ImageError::TooLarge`] or [`ImageError::Decode`]
/// from the backend.
pub fn decode_bytes(
    registry: &BackendRegistry,
    bytes: &[u8],
    request: &DecodeRequest,
) -> Result<DecodeResponse, ImageError> {
    let format = identify(bytes)?;
    registry.decoder_for(format).decode(format, bytes, request)
}

/// Header-only probe: the natural size and friends, without decoding pixels.
///
/// This is what layout waits on, and it is orders of magnitude cheaper than
/// [`decode_bytes`].
///
/// # Errors
///
/// As [`decode_bytes`], minus the decode-specific failures.
pub fn probe_bytes(registry: &BackendRegistry, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
    let format = identify(bytes)?;
    registry.decoder_for(format).probe(format, bytes)
}

/// Sniff plus the framing check, in the one order every entry point needs them.
fn identify(bytes: &[u8]) -> Result<ImageFormat, ImageError> {
    let format = sniff(bytes).ok_or(ImageError::UnknownFormat)?;
    if is_complete(format, bytes) {
        Ok(format)
    } else {
        Err(ImageError::Truncated {
            format,
            len: bytes.len(),
        })
    }
}
