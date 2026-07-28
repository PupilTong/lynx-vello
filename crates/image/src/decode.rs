//! The decode contract: what a caller asks for, what a backend returns, and the
//! trait every backend implements.

use std::fmt;

/// A two-dimensional physical-pixel size.
///
/// Re-exported from the resource protocol rather than redefined: it is already
/// the type [`ImageHints::target_size_px`](bobcat_engine::resource::ImageHints)
/// carries, so a second identical struct would only force every call site to
/// convert between them.
pub use bobcat_engine::resource::PixelSize;

use crate::error::ImageError;
use crate::format::ImageFormat;
use crate::pixels::DecodedImage;
use crate::registry::Acceleration;

/// Intrinsic metadata read from container headers, without decoding pixels.
///
/// This is what layout waits on. Probing is three to four orders of magnitude
/// cheaper than a full decode, which is the whole reason the natural size and
/// the pixels arrive on separate schedules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImageHeader {
    pub format: ImageFormat,
    /// Natural size in image pixels.
    ///
    /// v1 treats one image pixel as one CSS pixel: there are no density
    /// descriptors (`srcset`/`sizes` are not implemented), so no caller has a
    /// scale factor to divide by. When density lands, the conversion belongs
    /// here rather than at each call site.
    pub natural_size: PixelSize,
    pub has_alpha: bool,
    /// True for an animated WebP or APNG. v1 decodes frame 0 and reports this
    /// so a caller can tell a still image from a first frame.
    pub animated: bool,
}

/// Caps and targets applied to one decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeRequest {
    /// Decode-time downsample target in device px. `None` decodes at natural
    /// size. Backends that cannot scale during decode resample afterwards, so
    /// this is always honoured — but only the platform backends avoid paying
    /// peak full-size memory to do it.
    pub target_size: Option<PixelSize>,
    /// Hard per-axis rejection cap, defaulting to vello's 8192-px shared atlas
    /// bound. A rejection rather than a clamp: an image the atlas cannot
    /// allocate is silently not rendered, and a loud error beats a blank box.
    pub max_dimension: u32,
    /// Hard `width * height` cap, checked against the header probe before any
    /// decoder is constructed. This, not the decoder crates' own limits, is the
    /// real decode-bomb guard.
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
    ///
    /// A builder rather than struct-update syntax: this type is
    /// `#[non_exhaustive]`, so `..DecodeRequest::default()` is a hard error
    /// outside this crate, and adding a field later must not break callers.
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

    /// Rejects a header that would breach either cap, before anything is
    /// allocated.
    ///
    /// # Errors
    ///
    /// [`ImageError::TooLarge`] naming which cap was breached.
    pub fn check(&self, header: &ImageHeader) -> Result<(), ImageError> {
        let PixelSize { width, height } = header.natural_size;
        if width > self.max_dimension || height > self.max_dimension {
            return Err(ImageError::too_large(
                width,
                height,
                format!("max_dimension = {}", self.max_dimension),
            ));
        }
        if u64::from(width) * u64::from(height) > self.max_pixels {
            return Err(ImageError::too_large(
                width,
                height,
                format!("max_pixels = {}", self.max_pixels),
            ));
        }
        Ok(())
    }

    /// The size a decode should actually produce: the target clamped so it
    /// never *up*-samples, or the natural size when no target was asked for.
    ///
    /// Upscaling is deliberately refused. A target larger than the source only
    /// costs memory and bandwidth — `object-fit` scales the destination
    /// geometry at paint time, where the GPU does it for free.
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
#[non_exhaustive]
pub struct DecodeResponse {
    pub image: DecodedImage,
    /// The *source* metadata: `natural_size` is the image's own size, which is
    /// not `image`'s size when a decode-time downsample ran.
    pub header: ImageHeader,
    /// The tier the backend that actually ran reports for this format.
    pub acceleration: Acceleration,
    /// Which backend ran, for diagnostics and cache-provenance reporting.
    pub backend: &'static str,
}

/// One decode backend.
///
/// Implementations are stateless and thread-safe. Every platform handle
/// (`CGImageSource`, `IWICImagingFactory`, `AImageDecoder`) is created, used and
/// dropped inside a single call, because none of them is `Send` — only the
/// resulting `Vec<u8>` ever crosses a thread boundary.
pub trait Decoder: fmt::Debug + Send + Sync + 'static {
    /// Stable identifier, reported in [`DecodeResponse::backend`].
    fn name(&self) -> &'static str;

    /// Which formats this backend claims, and at which tier.
    fn capabilities(&self) -> crate::registry::Capabilities;

    /// Parses container headers only. Must not decode pixel data.
    ///
    /// # Errors
    ///
    /// [`ImageError::Decode`] when the headers are unreadable.
    fn probe(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError>;

    /// Decodes to RGBA8, honouring `request`'s caps and downsample target.
    ///
    /// # Errors
    ///
    /// [`ImageError::TooLarge`] when a cap is breached, [`ImageError::Decode`]
    /// otherwise.
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
    use crate::format::ImageFormat;

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

        let narrow = DecodeRequest {
            max_pixels: 1000,
            ..DecodeRequest::default()
        };
        let error = narrow
            .check(&header(100, 100))
            .expect_err("past max_pixels");
        assert!(format!("{error}").contains("max_pixels"));
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
        // A larger request clamps back to the source rather than upscaling.
        assert_eq!(request(400, 200).effective_size(natural), natural);
        // A degenerate target is ignored rather than producing a zero-area decode.
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
