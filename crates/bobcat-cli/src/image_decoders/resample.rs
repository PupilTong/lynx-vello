//! The shared post-decode resampler.

use bobcat_core::image::{AlphaType, ImageError, ImageFormat};

/// Resamples an RGBA8 buffer, alpha-correctly.
///
/// Shared by every implementation that cannot scale during decode — or, for the
/// platform decoders, cannot hit a non-aspect-preserving target during decode.
/// `use_alpha` is not optional: resampling straight-alpha pixels without
/// weighting by alpha bleeds fully transparent colour values into the visible
/// edge, which is exactly the halo browsers are careful not to produce.
pub(crate) fn resample(
    pixels: Vec<u8>,
    from: (u32, u32),
    to: (u32, u32),
    alpha_type: AlphaType,
    format: ImageFormat,
) -> Result<Vec<u8>, ImageError> {
    use fast_image_resize::images::Image;
    use fast_image_resize::{PixelType, ResizeOptions, Resizer};

    if from == to {
        return Ok(pixels);
    }
    let source = Image::from_vec_u8(from.0, from.1, pixels, PixelType::U8x4)
        .map_err(|error| ImageError::decode(format, format!("resample source: {error}")))?;
    let mut destination = Image::new(to.0, to.1, PixelType::U8x4);
    // Already-premultiplied pixels must not be premultiplied again; the
    // weighting `use_alpha` performs is exactly what premultiplication already
    // did.
    let options = ResizeOptions::new().use_alpha(alpha_type == AlphaType::Straight);
    Resizer::new()
        .resize(&source, &mut destination, &options)
        .map_err(|error| ImageError::decode(format, format!("resample: {error}")))?;
    Ok(destination.into_vec())
}
