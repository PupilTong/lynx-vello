//! Image decoding, done entirely by the platform.
//!
//! This crate contains no codec and links no decoding library. macOS decodes
//! through `ImageIO`, Linux through the desktop's gdk-pixbuf loaded at
//! runtime, and the browser through the main thread's `Image` element —
//! each the same decoder the platform's own image views use, with the same
//! format coverage, and each able to downsample *during* decode so a 4000px
//! photo shown at 200px never materialises at full size. What this module
//! owns is the shape every platform decoder answers in, [`Bitmap`], and the
//! one sizing rule they share.

use bobcat_core::ImageSizeHint;
use bobcat_core::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat as PixelFormat};

#[cfg(not(target_arch = "wasm32"))]
use crate::image_header::ImageHeader;

#[cfg(target_arch = "wasm32")]
pub(crate) mod browser;
#[cfg(all(unix, not(target_os = "macos")))]
mod gdk_pixbuf;
#[cfg(target_os = "macos")]
mod imageio;

/// Decoded pixels: tightly packed RGBA8, at most the size that was asked
/// for, with the image's own size beside them.
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// The image's intrinsic size — what layout is told — which the bitmap
    /// is a downsampled rendition of when the two differ.
    pub source_width: u32,
    pub source_height: u32,
    /// Whether the colour channels are premultiplied by alpha. `ImageIO`
    /// produces premultiplied pixels and gdk-pixbuf straight ones; vello
    /// composes either correctly as long as it is told which.
    pub premultiplied: bool,
    pub rgba: Vec<u8>,
}

impl Bitmap {
    /// The bytes the pixels occupy.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }

    /// The pixels as the image the compose path draws.
    #[must_use]
    pub fn into_image_data(self) -> ImageData {
        ImageData {
            data: Blob::from(self.rgba),
            format: PixelFormat::Rgba8,
            alpha_type: if self.premultiplied {
                ImageAlphaType::AlphaPremultiplied
            } else {
                ImageAlphaType::Alpha
            },
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum DecodeError {
    /// This platform has no decoder this crate knows how to reach.
    #[error("no platform image decoder is available: {0}")]
    Unavailable(String),
    /// The platform decoder does not decode this container.
    #[error("the platform decoder does not support this image: {0}")]
    Unsupported(String),
    /// The bytes are not a decodable image.
    #[error("the image could not be decoded: {0}")]
    Malformed(String),
}

/// The size a `source_width`x`source_height` image is decoded to under
/// `max`: inside the bound, the image's own ratio kept, never upsampled.
#[must_use]
pub fn target_size(source_width: u32, source_height: u32, max: (u32, u32)) -> (u32, u32) {
    ImageSizeHint::new(max.0, max.1).fit(source_width, source_height)
}

/// Decodes `bytes` with the platform decoder, downsampling to fit inside
/// `max` (per axis, in pixels). `header` is what the container's header said
/// about the image, if it could be read — the size a decoder that scales
/// during load needs before it has seen the pixels.
///
/// Blocking; runs on an IO worker or, for a restore, on the painter.
#[cfg(not(target_arch = "wasm32"))]
pub fn decode(
    bytes: &[u8],
    header: Option<ImageHeader>,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    #[cfg(target_os = "macos")]
    {
        imageio::decode(bytes, header, max)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        gdk_pixbuf::decode(bytes, header, max)
    }
    #[cfg(not(unix))]
    {
        let _ = (bytes, header, max);
        Err(DecodeError::Unavailable(
            "no platform decoder is wired for this operating system".to_owned(),
        ))
    }
}

/// Whether a platform decoder can be reached at all, so a host can report
/// the gap once at startup rather than once per image.
#[cfg(not(target_arch = "wasm32"))]
pub fn available() -> Result<(), DecodeError> {
    #[cfg(target_os = "macos")]
    {
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        gdk_pixbuf::available()
    }
    #[cfg(not(unix))]
    {
        Err(DecodeError::Unavailable(
            "no platform decoder is wired for this operating system".to_owned(),
        ))
    }
}

/// Reconciles a header's size with what the decoder produced: an EXIF
/// orientation the header probe cannot see may have transposed the image,
/// and the intrinsic size layout is told must be the oriented one.
///
/// gdk-pixbuf's helper: `ImageIO` reports the oriented size itself.
#[cfg(all(unix, not(target_os = "macos")))]
fn oriented_source_size(header: (u32, u32), decoded: (u32, u32)) -> (u32, u32) {
    let (header_landscape, decoded_landscape) = (header.0 > header.1, decoded.0 > decoded.1);
    if header.0 != header.1 && decoded.0 != decoded.1 && header_landscape != decoded_landscape {
        (header.1, header.0)
    } else {
        header
    }
}

/// Packs a decoder's rows — `channels` bytes per pixel, `stride` bytes per
/// row — into tightly packed RGBA8, filling alpha where the source has none.
///
/// gdk-pixbuf's helper: `ImageIO` draws straight into an RGBA8 context.
#[cfg(all(unix, not(target_os = "macos")))]
fn pack_rgba(
    pixels: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    channels: usize,
) -> Option<Vec<u8>> {
    if !(3..=4).contains(&channels) || stride < width.checked_mul(channels)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for row in 0..height {
        let start = row.checked_mul(stride)?;
        let source = pixels.get(start..start + width * channels)?;
        for pixel in source.chunks_exact(channels) {
            rgba.extend_from_slice(&pixel[..3]);
            rgba.push(if channels == 4 { pixel[3] } else { 255 });
        }
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bitmap_becomes_image_data_with_its_alpha_type() {
        let bitmap = Bitmap {
            width: 1,
            height: 1,
            source_width: 8,
            source_height: 8,
            premultiplied: true,
            rgba: vec![1, 2, 3, 4],
        };
        assert_eq!(bitmap.byte_len(), 4);
        let image = bitmap.into_image_data();
        assert_eq!(image.alpha_type, ImageAlphaType::AlphaPremultiplied);
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.data.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn the_target_size_fits_inside_the_bound_without_upsampling() {
        assert_eq!(target_size(4000, 3000, (200, 200)), (200, 150));
        assert_eq!(target_size(100, 50, (200, 200)), (100, 50));
        assert_eq!(target_size(0, 0, (200, 200)), (1, 1));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn rows_pack_from_rgb_and_rgba_with_stride() {
        let rgb = [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0];
        assert_eq!(
            pack_rgba(&rgb, 2, 2, 8, 3).unwrap(),
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        let rgba = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(pack_rgba(&rgba, 2, 1, 8, 4).unwrap(), rgba);
        assert!(
            pack_rgba(&rgba, 2, 1, 4, 4).is_none(),
            "a stride shorter than a row"
        );
        assert!(
            pack_rgba(&rgba, 2, 2, 8, 4).is_none(),
            "fewer rows than claimed"
        );
        assert!(
            pack_rgba(&rgba, 1, 1, 8, 2).is_none(),
            "an unknown channel count"
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_transposing_orientation_is_reconciled_from_the_decoded_shape() {
        assert_eq!(oriented_source_size((4000, 3000), (200, 150)), (4000, 3000));
        assert_eq!(oriented_source_size((4000, 3000), (150, 200)), (3000, 4000));
        assert_eq!(oriented_source_size((100, 100), (50, 80)), (100, 100));
    }
}

/// The platform decoder itself, driven with a PNG the `png` crate encodes.
///
/// These require the platform decoder — `ImageIO` on macOS, gdk-pixbuf on
/// Linux — and fail rather than skip when it is missing, so a green run
/// always means pixels were decoded and compared.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod platform_tests {
    use super::*;

    /// A width x height image whose quadrants are red, green, blue, and
    /// half-transparent white.
    fn quadrant_png(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let pixel = match (x < width / 2, y < height / 2) {
                    (true, true) => [255, 0, 0, 255],
                    (false, true) => [0, 255, 0, 255],
                    (true, false) => [0, 0, 255, 255],
                    (false, false) => [255, 255, 255, 128],
                };
                rgba.extend_from_slice(&pixel);
            }
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&rgba).expect("png data");
        }
        bytes
    }

    fn pixel(bitmap: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * bitmap.width + x) * 4) as usize;
        bitmap.rgba[start..start + 4]
            .try_into()
            .expect("four bytes")
    }

    /// Straight-alpha colour of a pixel, whichever way the platform stores it.
    fn straight(bitmap: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let [red, green, blue, alpha] = pixel(bitmap, x, y);
        if bitmap.premultiplied && alpha != 0 && alpha != 255 {
            let unmultiply = |channel: u8| {
                u8::try_from((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha))
                    .unwrap_or(255)
            };
            [unmultiply(red), unmultiply(green), unmultiply(blue), alpha]
        } else {
            [red, green, blue, alpha]
        }
    }

    fn close(actual: [u8; 4], expected: [u8; 4]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 3)
    }

    #[test]
    fn the_platform_decoder_is_present() {
        available().expect("this platform's image decoder must be reachable");
    }

    #[test]
    fn a_png_decodes_at_full_size_with_exact_colours() {
        let bytes = quadrant_png(64, 32);
        let header = crate::image_header::probe(&bytes);
        let bitmap = decode(&bytes, header, (u32::MAX, u32::MAX)).expect("decode");
        assert_eq!((bitmap.width, bitmap.height), (64, 32));
        assert_eq!((bitmap.source_width, bitmap.source_height), (64, 32));
        assert_eq!(bitmap.byte_len(), 64 * 32 * 4);
        assert!(close(straight(&bitmap, 0, 0), [255, 0, 0, 255]));
        assert!(close(straight(&bitmap, 63, 0), [0, 255, 0, 255]));
        assert!(close(straight(&bitmap, 0, 31), [0, 0, 255, 255]));
        assert!(close(straight(&bitmap, 63, 31), [255, 255, 255, 128]));
    }

    #[test]
    fn a_bound_downsamples_during_decode_and_keeps_the_intrinsic_size() {
        let bytes = quadrant_png(64, 32);
        for header in [crate::image_header::probe(&bytes), None] {
            let bitmap = decode(&bytes, header, (16, 16)).expect("decode");
            assert_eq!(
                (bitmap.width, bitmap.height),
                (16, 8),
                "header probe: {}",
                header.is_some()
            );
            assert_eq!((bitmap.source_width, bitmap.source_height), (64, 32));
            assert!(close(straight(&bitmap, 1, 1), [255, 0, 0, 255]));
            assert!(close(straight(&bitmap, 14, 6), [255, 255, 255, 128]));
        }
    }

    #[test]
    fn malformed_bytes_are_a_decode_error_not_a_crash() {
        assert!(matches!(
            decode(b"\x89PNG\r\n\x1a\nnot really", None, (64, 64)),
            Err(DecodeError::Malformed(_) | DecodeError::Unsupported(_))
        ));
        assert!(matches!(
            decode(b"", None, (64, 64)),
            Err(DecodeError::Malformed(_) | DecodeError::Unsupported(_))
        ));
    }
}
