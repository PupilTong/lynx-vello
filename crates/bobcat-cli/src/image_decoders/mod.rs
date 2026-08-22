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
//! - **Linux**: `SoftwareDecoder`, the pure-Rust reference (`png` + `zune-jpeg` + `image-webp`).
//!   Linux has no system still-image decode API; this is the only target that compiles it.
//!
//! Those two are the whole list. On any other target `platform_decoder` returns
//! `None` and there is no fallback behind it: an embedder shipping elsewhere
//! implements [`Decoder`] itself and injects it.

use std::sync::Arc;

use bobcat_core::image::Decoder;

mod resample;
pub(crate) use resample::resample;

#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
mod animation;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
mod orientation;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;
#[cfg(target_os = "linux")]
mod software;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use apple::AppleDecoder;
#[cfg(target_os = "linux")]
pub use software::SoftwareDecoder;

/// Returns the decoder selected for the current target when available.
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
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        None
    }
}
