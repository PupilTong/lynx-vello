//! The decoded byte format every decoder produces and the paint engine
//! consumes.

use dom::vello::peniko;

use crate::image::error::ImageError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaType {
    Straight,
    Premultiplied,
}

/// Decoded RGBA8 pixels plus the metadata needed to interpret them.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    data: peniko::Blob<u8>,
    width: u32,
    height: u32,
    alpha_type: AlphaType,
}

impl DecodedImage {
    /// Wraps a decoded buffer.
    pub fn from_rgba8(
        width: u32,
        height: u32,
        alpha_type: AlphaType,
        pixels: Vec<u8>,
        format: crate::image::format::ImageFormat,
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
            data: peniko::Blob::from(pixels),
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
        self.data.data()
    }

    /// Retained bytes, for the decode cache's byte budget.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.pixels().len()
    }

    /// The shared-buffer identity.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.data.id()
    }

    /// The `peniko` view of this image, for `dom`'s `ImageStore`.
    #[must_use]
    pub fn to_image_data(&self) -> peniko::ImageData {
        use peniko::{ImageAlphaType, ImageData, ImageFormat};

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

/// `4 * width * height`, or `None` on overflow.
#[must_use]
pub fn expected_byte_len(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|area| area.checked_mul(4))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{AlphaType, DecodedImage};
    use crate::image::format::ImageFormat;

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

    #[test]
    fn buffer_identity_survives_cloning_and_repeated_conversion() {
        let decoded = image(4, 4);
        let id = decoded.id();
        assert_eq!(decoded.clone().id(), id);
        assert_eq!(decoded.to_image_data().data.id(), id);
        assert_eq!(decoded.to_image_data().data.id(), id);
        assert_ne!(image(4, 4).id(), id);
    }

    #[test]
    fn alpha_type_maps_onto_peniko() {
        use super::peniko::{ImageAlphaType, ImageFormat as PenikoFormat};

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
