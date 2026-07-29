//! Decode backends: one always-compiled software implementation plus at most
//! one platform implementation, selected at runtime.
//!
//! The platform modules are `#[cfg]`'d to their own OS, so a build for one
//! target never so much as parses the other two. Selecting *whether* to use the
//! one that is compiled in is a separate, runtime question — see
//! [`platform_decoder`].

use std::sync::Arc;

use crate::decode::Decoder;

pub(crate) mod software;

#[cfg(target_os = "android")]
pub(crate) mod android;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod apple;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

/// The platform backend for this OS, if one is compiled in *and* the running
/// system actually provides it.
///
/// Returning `None` is a completely ordinary outcome, not a failure: the
/// software backend claims every supported format unconditionally, so a platform
/// backend is always an upgrade and never a dependency.
pub(crate) fn platform_decoder() -> Option<Arc<dyn Decoder>> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        apple::AppleDecoder::detect().map(|decoder| Arc::new(decoder) as Arc<dyn Decoder>)
    }
    #[cfg(target_os = "windows")]
    {
        windows::WicDecoder::detect().map(|decoder| Arc::new(decoder) as Arc<dyn Decoder>)
    }
    #[cfg(target_os = "android")]
    {
        android::NdkDecoder::detect().map(|decoder| Arc::new(decoder) as Arc<dyn Decoder>)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        // Linux and the BSDs have no system still-image decode API worth
        // preferring over the bundled codecs; there is nothing to probe for.
        None
    }
}

/// Resamples an RGBA8 buffer, alpha-correctly.
///
/// Shared by every backend that cannot scale during decode. `use_alpha` is not
/// optional: resampling straight-alpha pixels without weighting by alpha bleeds
/// fully transparent colour values into the visible edge, which is exactly the
/// halo browsers are careful not to produce.
pub(crate) fn resample(
    pixels: Vec<u8>,
    from: (u32, u32),
    to: (u32, u32),
    alpha_type: crate::pixels::AlphaType,
    format: crate::format::ImageFormat,
) -> Result<Vec<u8>, crate::error::ImageError> {
    use fast_image_resize::images::Image;
    use fast_image_resize::{PixelType, ResizeOptions, Resizer};

    use crate::error::ImageError;

    if from == to {
        return Ok(pixels);
    }
    let source = Image::from_vec_u8(from.0, from.1, pixels, PixelType::U8x4)
        .map_err(|error| ImageError::decode(format, format!("resample source: {error}")))?;
    let mut destination = Image::new(to.0, to.1, PixelType::U8x4);
    // Already-premultiplied pixels must not be premultiplied again; the
    // weighting `use_alpha` performs is exactly what premultiplication already
    // did.
    let options = ResizeOptions::new().use_alpha(alpha_type == crate::pixels::AlphaType::Straight);
    Resizer::new()
        .resize(&source, &mut destination, &options)
        .map_err(|error| ImageError::decode(format, format!("resample: {error}")))?;
    Ok(destination.into_vec())
}
