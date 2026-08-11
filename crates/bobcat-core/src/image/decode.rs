//! The decode contract: what a caller asks for, what a decoder returns, and the
//! trait the embedder-injected decoder implements.

use std::fmt;

use crate::image::capability::Acceleration;
use crate::image::error::ImageError;
use crate::image::format::ImageFormat;
use crate::image::pixels::DecodedImage;
pub use crate::resource::PixelSize;

/// Intrinsic metadata read from container headers, without decoding pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageHeader {
    pub format: ImageFormat,
    /// Natural size in image pixels.
    pub natural_size: PixelSize,
    pub has_alpha: bool,
    /// True for an animated WebP or APNG.
    pub animated: bool,
}

/// Caps and targets applied to one decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeRequest {
    /// Decode-time downsample target in device px.
    pub target_size: Option<PixelSize>,
    /// Hard per-axis cap on the **output**, defaulting to vello's 8192-px shared atlas bound.
    pub max_dimension: u32,
    /// Hard `width * height` cap on the **source**, checked against the header probe before any
    /// decoder is constructed.
    pub max_pixels: u64,
}

impl Default for DecodeRequest {
    fn default() -> Self {
        Self {
            target_size: None,
            max_dimension: 8192,
            max_pixels: 64 << 20,
        }
    }
}

impl DecodeRequest {
    /// Sets the decode-time downsample target.
    #[must_use]
    pub const fn with_target(mut self, target: Option<PixelSize>) -> Self {
        self.target_size = target;
        self
    }

    #[must_use]
    pub const fn with_max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = max_dimension;
        self
    }

    #[must_use]
    pub const fn with_max_pixels(mut self, max_pixels: u64) -> Self {
        self.max_pixels = max_pixels;
        self
    }

    /// Rejects a header whose source or output would breach its cap, before anything is allocated.
    pub fn check(&self, header: &ImageHeader) -> Result<(), ImageError> {
        let PixelSize { width, height } = header.natural_size;
        if u64::from(width) * u64::from(height) > self.max_pixels {
            return Err(ImageError::too_large(
                width,
                height,
                format!("max_pixels = {}", self.max_pixels),
            ));
        }
        let output = self.effective_size(header.natural_size);
        if output.width > self.max_dimension || output.height > self.max_dimension {
            return Err(ImageError::too_large(
                output.width,
                output.height,
                format!("max_dimension = {}", self.max_dimension),
            ));
        }
        Ok(())
    }

    /// The size a decode should actually produce: the target clamped so it never *up*-samples, or
    /// the natural size when no target was asked for.
    #[must_use]
    pub fn effective_size(&self, natural: PixelSize) -> PixelSize {
        let Some(target) = self.target_size else {
            return natural;
        };
        if target.width == 0 || target.height == 0 {
            return natural;
        }
        PixelSize {
            width: target.width.min(natural.width).max(1),
            height: target.height.min(natural.height).max(1),
        }
    }
}

/// One decoded image plus how it was produced.
#[derive(Clone, Debug)]
pub struct DecodeResponse {
    pub image: DecodedImage,
    /// The *source* metadata: `natural_size` is the image's own size, which is not `image`'s size
    /// when a decode-time downsample ran.
    pub header: ImageHeader,
    /// The tier the decoder reports for this format.
    pub acceleration: Acceleration,
    /// Which decoder ran, for diagnostics and cache-provenance reporting.
    pub backend: &'static str,
}

/// The decoder the embedder injects — the one point where pixels enter the engine.
pub trait Decoder: fmt::Debug + Send + Sync + 'static {
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> crate::image::capability::Capabilities;

    fn probe(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError>;

    fn decode(
        &self,
        format: ImageFormat,
        bytes: &[u8],
        request: &DecodeRequest,
    ) -> Result<DecodeResponse, ImageError>;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{DecodeRequest, ImageHeader, PixelSize};
    use crate::image::format::ImageFormat;

    fn header(width: u32, height: u32) -> ImageHeader {
        ImageHeader {
            format: ImageFormat::Png,
            natural_size: PixelSize { width, height },
            has_alpha: false,
            animated: false,
        }
    }

    #[test]
    fn check_rejects_past_either_cap() {
        let request = DecodeRequest::default();
        assert!(request.check(&header(4096, 4096)).is_ok());

        let error = request
            .check(&header(9000, 10))
            .expect_err("past max_dimension");
        assert!(format!("{error}").contains("max_dimension"));

        let error = DecodeRequest::default()
            .with_max_pixels(1000)
            .check(&header(100, 100))
            .expect_err("past max_pixels");
        assert!(format!("{error}").contains("max_pixels"));
    }

    #[test]
    fn the_axis_cap_applies_to_the_output_and_the_pixel_cap_to_the_source() {
        let downsampled = DecodeRequest::default()
            .with_max_dimension(8)
            .with_target(Some(PixelSize {
                width: 8,
                height: 1,
            }));
        assert!(
            downsampled.check(&header(64, 1)).is_ok(),
            "a 64x1 source decoded to 8x1 fits an 8px atlas bound"
        );
        assert!(
            DecodeRequest::default()
                .with_max_dimension(8)
                .check(&header(64, 1))
                .is_err()
        );

        let bomb = DecodeRequest::default()
            .with_max_pixels(100)
            .with_target(Some(PixelSize {
                width: 2,
                height: 2,
            }));
        assert!(
            bomb.check(&header(5000, 5000)).is_err(),
            "a downsample target must not defuse the decode-bomb guard"
        );
    }

    #[test]
    fn effective_size_never_upsamples() {
        let natural = PixelSize {
            width: 100,
            height: 50,
        };
        let request = |width, height| DecodeRequest {
            target_size: Some(PixelSize { width, height }),
            ..DecodeRequest::default()
        };

        assert_eq!(
            request(40, 20).effective_size(natural),
            PixelSize {
                width: 40,
                height: 20
            }
        );
        assert_eq!(request(400, 200).effective_size(natural), natural);
        assert_eq!(request(0, 20).effective_size(natural), natural);
        assert_eq!(DecodeRequest::default().effective_size(natural), natural);
    }

    #[test]
    fn effective_size_keeps_at_least_one_pixel_per_axis() {
        let natural = PixelSize {
            width: 100,
            height: 50,
        };
        let tiny = DecodeRequest {
            target_size: Some(PixelSize {
                width: 1,
                height: 1,
            }),
            ..DecodeRequest::default()
        };
        assert_eq!(
            tiny.effective_size(natural),
            PixelSize {
                width: 1,
                height: 1
            }
        );
    }
}
