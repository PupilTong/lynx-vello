//! The decoded byte format every backend produces and the paint engine consumes.

#[cfg(not(feature = "vello"))]
use std::sync::Arc;

use crate::error::ImageError;

/// How a decoded buffer encodes alpha.
///
/// Carried rather than normalised. The software and WIC paths emit straight
/// alpha, `ImageIO` and `AImageDecoder` premultiplied; converting on the CPU would
/// be pure loss, because vello's fine shader premultiplies per texel before
/// filtering either way. Byte-identical output across backends is therefore not
/// a goal — identical *composited* output is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaType {
    /// Separate (straight, unpremultiplied) alpha channel.
    Straight,
    /// Colour channels already scaled by alpha.
    Premultiplied,
}

/// The pixel buffer, behind whichever shared handle the build has available.
///
/// With the `vello` feature on this *is* a `peniko::Blob`, so the identity
/// `peniko` mints at construction is the identity every
/// [`DecodedImage::to_image_data`] call hands back. That is what keeps vello's
/// atlas holding one residency entry for the image instead of re-uploading it
/// every frame, and making it structural beats leaving it to a convention a
/// later caller can quietly break.
#[cfg(feature = "vello")]
type Buffer = vello::peniko::Blob<u8>;
#[cfg(not(feature = "vello"))]
type Buffer = Arc<[u8]>;

/// Decoded RGBA8 pixels plus the metadata needed to interpret them.
///
/// Row-major and tightly packed: stride is exactly `4 * width` with no row
/// padding, length exactly `4 * width * height`, channel order R, G, B, A in
/// memory.
///
/// Colour values are the file's own sRGB-encoded bytes. No ICC or CICP
/// conversion and no gamma conversion is performed, because vello's atlas is
/// `Rgba8Unorm` rather than `Rgba8UnormSrgb` — a wide-gamut or tagged image
/// renders as if it were sRGB.
///
/// Cloning is cheap: the buffer is shared, not copied.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    data: Buffer,
    width: u32,
    height: u32,
    alpha_type: AlphaType,
}

impl DecodedImage {
    /// Wraps a decoded buffer.
    ///
    /// # Errors
    ///
    /// [`ImageError::Decode`] when either axis is zero or `pixels.len()` is not
    /// exactly `4 * width * height` — a backend that miscounts its own stride
    /// must not reach the atlas.
    pub fn from_rgba8(
        width: u32,
        height: u32,
        alpha_type: AlphaType,
        pixels: Vec<u8>,
        format: crate::format::ImageFormat,
    ) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::decode(
                format,
                format!("decoded to a zero-area image ({width}x{height})"),
            ));
        }
        let expected = expected_byte_len(width, height).ok_or_else(|| {
            ImageError::too_large(width, height, "width * height * 4 overflows usize")
        })?;
        if pixels.len() != expected {
            return Err(ImageError::decode(
                format,
                format!(
                    "decoded buffer is {} bytes, expected {expected} for {width}x{height} RGBA8",
                    pixels.len()
                ),
            ));
        }
        Ok(Self {
            data: buffer_from(pixels),
            width,
            height,
            alpha_type,
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub const fn alpha_type(&self) -> AlphaType {
        self.alpha_type
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        #[cfg(feature = "vello")]
        {
            self.data.data()
        }
        #[cfg(not(feature = "vello"))]
        {
            &self.data
        }
    }

    /// Retained bytes, for the decode cache's byte budget.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.pixels().len()
    }

    /// The shared-buffer identity. Stable across [`Clone`] and across repeated
    /// [`Self::to_image_data`] calls, which is the property that stops vello
    /// re-uploading the same image every frame.
    #[cfg(feature = "vello")]
    #[must_use]
    pub fn id(&self) -> u64 {
        self.data.id()
    }

    /// The `peniko` view of this image, for
    /// `dom`'s `ImageStore`.
    ///
    /// Cheap — it clones the shared buffer handle, never the pixels.
    #[cfg(feature = "vello")]
    #[must_use]
    pub fn to_image_data(&self) -> vello::peniko::ImageData {
        use vello::peniko::{ImageAlphaType, ImageData, ImageFormat};

        ImageData {
            data: self.data.clone(),
            format: ImageFormat::Rgba8,
            alpha_type: match self.alpha_type {
                AlphaType::Straight => ImageAlphaType::Alpha,
                AlphaType::Premultiplied => ImageAlphaType::AlphaPremultiplied,
            },
            width: self.width,
            height: self.height,
        }
    }
}

fn buffer_from(pixels: Vec<u8>) -> Buffer {
    #[cfg(feature = "vello")]
    {
        vello::peniko::Blob::from(pixels)
    }
    #[cfg(not(feature = "vello"))]
    {
        Arc::from(pixels)
    }
}

/// `4 * width * height`, or `None` on overflow.
pub(crate) fn expected_byte_len(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|area| area.checked_mul(4))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{AlphaType, DecodedImage};
    use crate::format::ImageFormat;

    fn image(width: u32, height: u32) -> DecodedImage {
        DecodedImage::from_rgba8(
            width,
            height,
            AlphaType::Straight,
            vec![0u8; (width * height * 4) as usize],
            ImageFormat::Png,
        )
        .expect("well-formed buffer")
    }

    #[test]
    fn accepts_an_exactly_sized_buffer() {
        let decoded = image(3, 2);
        assert_eq!(decoded.width(), 3);
        assert_eq!(decoded.height(), 2);
        assert_eq!(decoded.pixels().len(), 3 * 2 * 4);
        assert_eq!(decoded.byte_len(), 24);
    }

    #[test]
    fn rejects_a_mismatched_buffer_length() {
        let error =
            DecodedImage::from_rgba8(2, 2, AlphaType::Straight, vec![0u8; 15], ImageFormat::Png)
                .expect_err("15 bytes cannot be 2x2 RGBA8");
        assert!(format!("{error}").contains("expected 16"));
    }

    #[test]
    fn rejects_a_zero_axis() {
        for (width, height) in [(0, 4), (4, 0)] {
            DecodedImage::from_rgba8(
                width,
                height,
                AlphaType::Straight,
                Vec::new(),
                ImageFormat::Png,
            )
            .expect_err("a zero-area image must not reach the atlas");
        }
    }

    #[cfg(feature = "vello")]
    #[test]
    fn buffer_identity_survives_cloning_and_repeated_conversion() {
        // The anti-re-upload invariant: vello's atlas keys residency on the
        // blob id, so every view of one decoded image must share one id.
        let decoded = image(4, 4);
        let id = decoded.id();
        assert_eq!(decoded.clone().id(), id);
        assert_eq!(decoded.to_image_data().data.id(), id);
        assert_eq!(decoded.to_image_data().data.id(), id);
        // A separately decoded image is a different entry.
        assert_ne!(image(4, 4).id(), id);
    }

    #[cfg(feature = "vello")]
    #[test]
    fn alpha_type_maps_onto_peniko() {
        use vello::peniko::{ImageAlphaType, ImageFormat as PenikoFormat};

        let straight = image(1, 1).to_image_data();
        assert_eq!(straight.alpha_type, ImageAlphaType::Alpha);
        assert_eq!(straight.format, PenikoFormat::Rgba8);
        assert_eq!((straight.width, straight.height), (1, 1));

        let premultiplied = DecodedImage::from_rgba8(
            1,
            1,
            AlphaType::Premultiplied,
            vec![0u8; 4],
            ImageFormat::Png,
        )
        .expect("well-formed buffer")
        .to_image_data();
        assert_eq!(premultiplied.alpha_type, ImageAlphaType::AlphaPremultiplied);
    }
}
