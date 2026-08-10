//! The decode contract: what a caller asks for, what a decoder returns, and the
//! trait the embedder-injected decoder implements.

use std::fmt;

use crate::image::capability::Acceleration;
use crate::image::error::ImageError;
use crate::image::format::ImageFormat;
use crate::image::pixels::DecodedImage;
/// A two-dimensional physical-pixel size.
///
/// Re-exported from the resource protocol rather than redefined: it is already
/// the type [`ImageHints::target_size_px`](crate::resource::ImageHints)
/// carries, so a second identical struct would only force every call site to
/// convert between them.
pub use crate::resource::PixelSize;

/// Intrinsic metadata read from container headers, without decoding pixels.
///
/// This is what layout waits on. Probing is three to four orders of magnitude
/// cheaper than a full decode, which is the whole reason the natural size and
/// the pixels arrive on separate schedules.
///
/// Not `#[non_exhaustive]`: decoder implementations live *outside* this crate
/// and construct this by struct literal, which that attribute forbids across a
/// crate boundary. Adding a field is a breaking change for every injected
/// decoder — deliberately, since a new header field is new contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Hard per-axis cap on the **output**, defaulting to vello's 8192-px shared
    /// atlas bound.
    ///
    /// Checked against the size the decode will actually produce, not the
    /// source: the two limits guard different things. This one exists because
    /// the atlas cannot allocate a larger texture, and a downsampled decode
    /// never asks it to — rejecting a 64x1 source that was being decoded to 8x1
    /// would refuse an image the renderer handles perfectly well.
    pub max_dimension: u32,
    /// Hard `width * height` cap on the **source**, checked against the header
    /// probe before any decoder is constructed.
    ///
    /// This is the decode-bomb guard, and it belongs on the source because the
    /// bomb is the decompression itself: a downsample target does not make a
    /// 50000x50000 input cheap to decode.
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

    /// Rejects a header whose source or output would breach its cap, before
    /// anything is allocated.
    ///
    /// # Errors
    ///
    /// [`ImageError::TooLarge`] naming which cap was breached.
    pub fn check(&self, header: &ImageHeader) -> Result<(), ImageError> {
        let PixelSize { width, height } = header.natural_size;
        // Decode-bomb guard: the source is what gets decompressed.
        if u64::from(width) * u64::from(height) > self.max_pixels {
            return Err(ImageError::too_large(
                width,
                height,
                format!("max_pixels = {}", self.max_pixels),
            ));
        }
        // Atlas guard: what the renderer is asked to hold.
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
///
/// Not `#[non_exhaustive]`, for [`ImageHeader`]'s reason: decoder
/// implementations outside this crate construct it by struct literal.
#[derive(Clone, Debug)]
pub struct DecodeResponse {
    pub image: DecodedImage,
    /// The *source* metadata: `natural_size` is the image's own size, which is
    /// not `image`'s size when a decode-time downsample ran.
    pub header: ImageHeader,
    /// The tier the decoder reports for this format.
    pub acceleration: Acceleration,
    /// Which decoder ran, for diagnostics and cache-provenance reporting.
    pub backend: &'static str,
}

/// The decoder the embedder injects — the one point where pixels enter the
/// engine.
///
/// No implementation ships with the engine. `image-decoders` provides the
/// sanctioned ones (Apple `ImageIO`, Windows WIC, Android `AImageDecoder`, and
/// the Linux-only pure-Rust reference), and an embedder with its own image
/// pipeline may implement this trait instead.
///
/// # Contract
///
/// - Implementations are stateless and thread-safe: [`probe`](Self::probe) and
///   [`decode`](Self::decode) are called concurrently from arbitrary worker threads. Every platform
///   handle (`CGImageSource`, `IWICImagingFactory`, `AImageDecoder`) must be created, used and
///   dropped inside a single call, because none of them is `Send` — only plain buffers may cross
///   threads.
/// - [`ImageHeader::natural_size`] is reported in **oriented** space: a JPEG whose EXIF tag rotates
///   it presents its rotated dimensions, and [`decode`](Self::decode) returns pixels in that same
///   space. Layout consumes the probe, so a decoder that disagrees with its own probe shows up as a
///   mis-sized box.
/// - [`decode`](Self::decode) honours [`DecodeRequest`]'s caps and downsample target exactly:
///   output size is [`DecodeRequest::effective_size`], never an upscale, and a breached cap is
///   [`ImageError::TooLarge`] *before* pixels are allocated.
/// - Neither call is invoked for a format [`capabilities`](Self::capabilities) does not claim —
///   [`decode_bytes`](crate::image::decode_bytes) and the loader refuse those as
///   [`ImageError::Unsupported`] first.
pub trait Decoder: fmt::Debug + Send + Sync + 'static {
    /// Stable identifier, reported in [`DecodeResponse::backend`].
    fn name(&self) -> &'static str;

    /// Which formats this decoder claims, and at which tier.
    fn capabilities(&self) -> crate::image::capability::Capabilities;

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
        // A source larger than the axis cap is fine when it is being decoded
        // down below it: `max_dimension` guards what the atlas must hold, and a
        // downsampled decode never asks the atlas for the source size.
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
        // Without the downsample it is genuinely too wide.
        assert!(
            DecodeRequest::default()
                .with_max_dimension(8)
                .check(&header(64, 1))
                .is_err()
        );

        // The pixel cap is not escapable by asking for a small target: the bomb
        // is the decompression, which happens at source resolution.
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
