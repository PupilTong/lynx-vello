//! The image decode contract and the async loader; the decoder itself is
//! injected.
//!
//! This module owns the replaced-content pipeline below the DOM: it identifies
//! a container from its leading bytes ([`sniff`]), verifies the container's own
//! framing ([`is_complete`]), and internally drives fetch→decode→cache from a
//! host [`ResourceFetcher`](crate::resource::ResourceFetcher) with byte budgets,
//! cancellation, and bounded caches for both the pixels and the much cheaper
//! natural sizes. The loader and its caches are engine-owned implementation
//! details; what stays outside is the codec contract in this module.
//!
//! **No codec ships here.** Decoding happens behind the [`Decoder`] trait, and
//! the embedder implements that codec contract. Embedders can install decoded
//! CSS URL pixels through [`LynxView::register_image_url`](crate::LynxView::register_image_url)
//! without gaining access to the private document or paint registry. The
//! view-level `<image>` element integration (natural-size installation,
//! request arbitration, and events) is not wired yet, so current consumers
//! call [`decode_bytes`] directly. The reference implementations live in the
//! reference embedder, `bobcat-cli`'s `image_decoders` module:
//!
//! - **Apple** (macOS/iOS): `ImageIO`, claiming PNG, JPEG, WebP, GIF, HEIC and AVIF. The system
//!   decoder is the *only* decoder on these targets.
//! - **Windows**: WIC — PNG and JPEG inbox, WebP when the Store extension is installed.
//! - **Android**: NDK `AImageDecoder` (API 30+), reached via `dlopen`.
//! - **Linux**: the pure-Rust reference decoder (`png` + `zune-jpeg` + `image-webp`), compiled for
//!   that OS only. It exists so headless Linux hosts decode at all; it is not shipped on the
//!   platforms above.
//!
//! An embedder with its own image pipeline (an app already running `SDWebImage`,
//! `Fresco` or similar) implements [`Decoder`] over that pipeline instead and
//! injects it through the same codec seam — that is the point of the boundary.
//!
//! This module deliberately never touches `dom`'s node types. It returns an
//! [`ImageHeader`] and a [`DecodedImage`]; installing the natural size on a
//! node remains the future element loop's job, while URL-keyed decoded pixels
//! enter the private paint-side store through the narrow view capability.
//!
//! # What "hardware decoding" actually means here
//!
//! [`Acceleration`] reports codec **provenance**, not silicon. No still-image
//! API on any supported platform exposes an acceleration query, and none of
//! them reaches a dedicated decode ASIC, so [`Acceleration::DedicatedHardware`]
//! is reserved and never reported. Claiming otherwise would be the easy lie; the
//! ladder we can actually observe is the honest answer.
//!
//! # Deliberate v1 limits (the compat bar is behavioral, not pixel-perfect — see AGENTS.md)
//!
//! 1. Six containers are *identified* — PNG, JPEG, WebP, GIF, HEIC, AVIF — and the injected
//!    decoder's [`Capabilities`] decide which of them decode. The Linux reference decoder claims
//!    the first three; the Apple decoder claims all six. A sniffed format the decoder does not
//!    claim is [`ImageError::Unsupported`]. SVG is not identified at all.
//! 2. Static only. An animated container (GIF, animated WebP, APNG, animation-brand HEIC/AVIF)
//!    decodes to frame 0 with [`ImageHeader::animated`] set and the rest discarded. No loop count
//!    or frame delay is retained, so Lynx's `loop-count`/`startplay`/`currentloopcomplete` surface
//!    has nothing to read even once an element layer exists.
//! 3. No decoder reaches a decode ASIC, so [`Acceleration`] is a provenance ladder rather than a
//!    claim about silicon, and its top rung is unreported.
//! 4. Only the platform decoders downsample *during* decode. The Linux reference decoder decodes at
//!    full resolution and resamples, so it pays peak full-size memory for a small retained bitmap —
//!    a memory profile difference, not a behavioral one.
//! 5. [`DecodeRequest::max_dimension`] defaults to 8192 and is a hard rejection rather than a
//!    clamp, because vello packs every scene image into one shared atlas capped at that size and an
//!    image it cannot allocate is silently not rendered.
//! 6. Colour management is not performed. Decoded bytes are the file's own sRGB-encoded values with
//!    no ICC or CICP conversion and no gamma conversion, because vello's atlas is `Rgba8Unorm`
//!    rather than `Rgba8UnormSrgb`. A wide-gamut or tagged image renders as if it were sRGB — HEIC
//!    and AVIF sources, which are routinely wide-gamut, included.
//! 7. Alpha encoding is carried, not normalised — see [`AlphaType`]. Byte-identical output across
//!    decoders is not a goal; identical composited output is.
//! 8. EXIF orientation is honoured for JPEG (all decoders, via the shared byte parser) and for
//!    HEIC/AVIF (Apple decoder, via `ImageIO`'s orientation property). PNG's `eXIf` chunk and
//!    WebP's `EXIF` chunk are not read.
//! 9. One node owns exactly one in-flight request. Lynx `<image>`'s concurrent src/placeholder
//!    race, its last-wins arbitration and its "src success permanently suppresses placeholder" lock
//!    are element policy that belongs above this module; all this module owes upward is per-request
//!    cancellation.
//! 10. No retry and no backoff. One failed fetch or decode is one terminal error, matching native
//!     Lynx, where no retry path exists anywhere; the resource protocol's `RetryAdvice` already
//!     defaults to `Never`.
//! 11. Decode cancellation is cooperative on the async side only. A decode already inside a
//!     blocking task cannot be aborted — the platform decoders are single one-shot C calls — so a
//!     cancelled load returns promptly while the decode drains and its result is discarded. Decode
//!     concurrency is bounded so drained work cannot starve the pool.
//! 12. Errors are a typed enum, not Lynx's `error_code`/`lynx_categorized_code` integers. See
//!     [`ImageError`].
//! 13. `cap-insets` (9-slice), `blur-radius`, `region-to-decode`, `tint-color` and the iOS fade-in
//!     are absent — all Lynx `<image>` extensions with no W3C `<img>` counterpart.
//! 14. One image pixel is one CSS pixel. There are no density descriptors (`srcset`/`sizes` are not
//!     implemented), so nothing here needs a scale factor; when density lands, the conversion
//!     belongs on [`ImageHeader`] rather than at each call site.
//! 15. The WIC and `AImageDecoder` reference decoders are unexecuted, and since they moved into the
//!     CLI — whose mandatory `QuickJS` C sources do not cross-compile to those ABIs — no CI gate
//!     type-checks them either. They are recorded reference material for embedders that do not
//!     exist yet. On those targets a failed capability probe leaves the embedder with no decoder at
//!     all (the reference decoder is Linux-only); shipping there means accepting that or injecting
//!     an embedder-side fallback.

// Retained as an engine-owned pipeline while the Lynx `<image>` element is wired above it.
#[allow(dead_code)]
mod cache;
mod capability;
#[allow(dead_code)]
mod data_url;
mod decode;
mod error;
mod format;
#[allow(dead_code)]
mod loader;
mod pixels;

pub use capability::{Acceleration, Capabilities};
pub use decode::{DecodeRequest, DecodeResponse, Decoder, ImageHeader, PixelSize};
pub use error::ImageError;
pub use format::{ImageFormat, is_complete, sniff};
pub use pixels::{AlphaType, DecodedImage, expected_byte_len};

#[cfg(test)]
mod loader_tests;

/// Identifies, validates and decodes one in-memory image with the injected decoder.
pub fn decode_bytes(
    decoder: &dyn Decoder,
    bytes: &[u8],
    request: &DecodeRequest,
) -> Result<DecodeResponse, ImageError> {
    let format = identify(decoder, bytes)?;
    decoder.decode(format, bytes, request)
}

/// Header-only probe: the natural size and friends, without decoding pixels.
pub fn probe_bytes(decoder: &dyn Decoder, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
    let format = identify(decoder, bytes)?;
    decoder.probe(format, bytes)
}

fn identify(decoder: &dyn Decoder, bytes: &[u8]) -> Result<ImageFormat, ImageError> {
    let format = sniff(bytes).ok_or(ImageError::UnknownFormat)?;
    if !decoder.capabilities().supports(format) {
        return Err(ImageError::Unsupported { format });
    }
    if is_complete(format, bytes) {
        Ok(format)
    } else {
        Err(ImageError::Truncated {
            format,
            len: bytes.len(),
        })
    }
}
