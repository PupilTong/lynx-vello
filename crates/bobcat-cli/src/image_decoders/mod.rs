//! The reference [`Decoder`] implementations this embedder injects into the
//! engine's image loader.
//!
//! The engine only *designs* the contract — [`Decoder`], `DecodeRequest`,
//! `Capabilities`, in `bobcat_core::image` — and ships no codec. Implementing
//! it is embedder work, which is why these decoders live here rather than in a
//! shared crate: an embedder with its own image pipeline implements `Decoder`
//! over that pipeline and never sees this module. One implementation per OS,
//! selected at *compile time* by target:
//!
//! - **Apple** (macOS/iOS): `AppleDecoder`, `ImageIO`. Claims PNG, JPEG, WebP, GIF, HEIC and AVIF
//!   unconditionally — this workspace assumes an OS recent enough to carry all six codecs, so there
//!   is no runtime probe.
//! - **Windows**: `WicDecoder`, the Windows Imaging Component. PNG and JPEG are inbox; WebP is a
//!   Store extension, so a runtime probe decides per format.
//! - **Android**: `NdkDecoder`, the NDK's `AImageDecoder`, reached through `dlopen` because the API
//!   is 30+ and the workspace minimum is lower.
//! - **Linux**: `SoftwareDecoder`, the pure-Rust reference (`png` + `zune-jpeg` + `image-webp`).
//!   Linux has no system still-image decode API; this is the only target that compiles it.
//!
//! The Windows and Android modules are carried for the embedder that does not
//! exist yet: this CLI builds for macOS and Linux, so they compile on no
//! supported target and no CI gate reaches them — they are reference material,
//! reviewed when they were written, not live code.

use std::sync::Arc;

use bobcat_core::image::Decoder;

mod resample;
pub(crate) use resample::resample;

// Compiled unconditionally so its tests run wherever the test suite does, even
// though only the Apple and Android decoders consult it — hence the dead-code
// allowance on the other targets.
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

/// The decoder this embedder injects on the running OS.
///
/// `None` is possible only where a runtime probe can fail: Windows without the
/// imaging components (Nano Server), Android below API 30. There is no
/// fallback behind it — the Linux reference decoder is deliberately compiled
/// for Linux alone — so on those targets the embedder ships without image
/// decoding or replaces this module.
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
