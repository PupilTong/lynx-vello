//! The sanctioned [`Decoder`] implementations an embedder injects into the
//! engine's image loader.
//!
//! The engine owns the contract — [`Decoder`], `DecodeRequest`,
//! `Capabilities`, in `bobcat_core::image`, re-exported here as [`contract`] —
//! and ships no codec. This crate is where the codecs live, one implementation
//! per OS, selected at *compile time* by target:
//!
//! - **Apple** (macOS/iOS): `AppleDecoder`, `ImageIO`. Claims PNG, JPEG, WebP, GIF, HEIC and AVIF
//!   unconditionally — this workspace assumes an OS recent enough to carry all six codecs, so there
//!   is no runtime probe.
//! - **Windows**: `WicDecoder`, the Windows Imaging Component. PNG and JPEG are inbox; WebP is a
//!   Store extension, so a runtime probe decides per format.
//! - **Android**: `NdkDecoder`, the NDK's `AImageDecoder`, reached through `dlopen` because the API
//!   is 30+ and the workspace minimum is lower.
//! - **Linux**: `SoftwareDecoder`, the pure-Rust reference (`png` + `zune-jpeg` + `image-webp`).
//!   Linux has no system still-image decode API, and headless CI runs there; this is the only
//!   target that compiles it.
//!
//! An embedder that wants none of these — because it already owns an image
//! pipeline — implements `Decoder` itself and never links this crate. That is
//! the seam working as intended, not a workaround.

// The coverage run compiles with `--cfg coverage_nightly` and the test modules
// opt out via `#[coverage(off)]`, which needs this experimental feature (same
// pattern as every other workspace crate).
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::sync::Arc;

/// The decode contract this crate implements, re-exported so a consumer that
/// only wants "a decoder plus the types to drive it" — a test, a tool, a layer
/// that must not name the engine crate directly — needs exactly one
/// dependency.
pub use bobcat_core::image as contract;
use bobcat_core::image::Decoder;

mod resample;
pub(crate) use resample::resample;

// Compiled unconditionally so its tests run wherever the test suite does
// (Linux CI included), even though only the Apple and Android decoders consult
// it — hence the dead-code allowance on the other targets.
#[cfg_attr(
    not(any(target_os = "android", target_os = "macos", target_os = "ios")),
    allow(dead_code)
)]
mod animation;

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "windows"
))]
mod orientation;

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;
#[cfg(target_os = "linux")]
mod software;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "android")]
pub use android::NdkDecoder;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use apple::AppleDecoder;
#[cfg(target_os = "linux")]
pub use software::SoftwareDecoder;
#[cfg(target_os = "windows")]
pub use windows::WicDecoder;

/// The decoder an embedder on this OS injects, unless it brings its own.
///
/// `None` is possible only where a runtime probe can fail: Windows without the
/// imaging components (Nano Server), Android below API 30. There is no
/// fallback behind it — the reference decoder is deliberately Linux-only — so
/// an embedder shipping to those environments either accepts images not
/// decoding or injects its own [`Decoder`].
#[must_use]
pub fn platform_decoder() -> Option<Arc<dyn Decoder>> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        Some(Arc::new(AppleDecoder::new()) as Arc<dyn Decoder>)
    }
    #[cfg(target_os = "linux")]
    {
        Some(Arc::new(SoftwareDecoder::new()) as Arc<dyn Decoder>)
    }
    #[cfg(target_os = "windows")]
    {
        WicDecoder::detect().map(|decoder| Arc::new(decoder) as Arc<dyn Decoder>)
    }
    #[cfg(target_os = "android")]
    {
        NdkDecoder::detect().map(|decoder| Arc::new(decoder) as Arc<dyn Decoder>)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        None
    }
}
