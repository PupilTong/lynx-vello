//! The Apple platform backend: `ImageIO`, on macOS and iOS.
//!
//! `ImageIO` is reached for one concrete reason —
//! `CGImageSourceCreateThumbnailAtIndex` scales *during* decode, which for JPEG
//! means the IDCT runs at 1/2, 1/4 or 1/8 scale in the DCT domain instead of
//! decoding a full-size bitmap only to throw most of it away. That is the whole
//! value proposition here; everything else in this file exists to make `ImageIO`'s
//! output agree, byte-shape for byte-shape, with
//! [`SoftwareDecoder`](crate::SoftwareDecoder).
//!
//! # Why [`Acceleration::PlatformSoftware`] and never `DedicatedHardware`
//!
//! No still-image API on this platform reaches a decode ASIC and none of them
//! exposes an acceleration query. `ImageIO`'s JPEG path imports `vImage` and no
//! `IOKit` symbols — it is Apple's own vendor-tuned CPU codec, which is exactly
//! what [`Acceleration::PlatformSoftware`] means. There is no observable signal
//! that would justify claiming the reserved
//! [`DedicatedHardware`](Acceleration::DedicatedHardware) rung, so this backend
//! never reports it.
//!
//! # Thread safety
//!
//! `CGImageSource`, `CGImage` and `CGContext` are thread-safe Core Foundation
//! types with no main-thread requirement, and this backend never leans on that
//! anyway: every handle is created, used and dropped inside a single method
//! call, so nothing Objective-C-shaped ever escapes. The only value that crosses
//! a thread boundary is the plain `Vec<u8>` of decoded pixels. That is what lets
//! [`AppleDecoder`] be `Send + Sync` without holding any platform state.
//!
//! # Unsafe
//!
//! Every `ImageIO` and Core Graphics entry point is a raw `extern "C"` binding, so
//! the `unsafe` in this file is unavoidable and falls into exactly four
//! categories, each individually justified at its use site:
//!
//! 1. Reading the `extern "C"` `CFString` constants `ImageIO` and Core Graphics export as
//!    dictionary keys (`kCGImageProperty*`, `kCGImageSource*`, `kCGColorSpaceSRGB`).
//! 2. Calling the `unsafe fn` wrappers `objc2` generates for functions taking an options
//!    dictionary, whose safety contract is "the dictionary's generics must be of the correct type".
//! 3. `CFRetained::cast_unchecked`, to give the untyped `CFArray`/`CFDictionary` that `ImageIO`
//!    returns the element types its documentation promises.
//! 4. `CGBitmapContextCreate`, which borrows a caller-owned pixel buffer for the lifetime of the
//!    context.
//!
//! There is no raw pointer arithmetic, no manual retain/release (`CFRetained`
//! owns every handle) and no transmute.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::OnceLock;

use objc2_core_foundation::{
    CFArray, CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint,
    CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_image_io::{
    CGImageSource, kCGImagePropertyHasAlpha, kCGImagePropertyPixelHeight,
    kCGImagePropertyPixelWidth, kCGImageSourceCreateThumbnailFromImageAlways,
    kCGImageSourceThumbnailMaxPixelSize,
};

use crate::backend::resample;
use crate::decode::{DecodeRequest, DecodeResponse, Decoder, ImageHeader, PixelSize};
use crate::error::ImageError;
use crate::format::ImageFormat;
use crate::orientation::{self, Orientation};
use crate::pixels::{AlphaType, DecodedImage, expected_byte_len};
use crate::registry::{Acceleration, Capabilities, probe_once};

/// PNG, JPEG and WebP through `ImageIO`, when the running system's codec list
/// actually offers them.
///
/// The probed capability set is stored rather than re-derived, so
/// [`Decoder::capabilities`] is a field read; the probe behind it is memoised
/// process-wide regardless.
#[derive(Clone, Copy, Debug)]
pub struct AppleDecoder {
    capabilities: Capabilities,
}

impl AppleDecoder {
    /// The runtime probe. `None` when `ImageIO` offers none of the three
    /// containers this crate decodes.
    ///
    /// `None` is an ordinary outcome, not a failure: the software backend claims
    /// all three unconditionally, so this backend is an upgrade and never a
    /// dependency.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let capabilities = probe_capabilities();
        if capabilities == Capabilities::none() {
            return None;
        }
        Some(Self { capabilities })
    }
}

impl Decoder for AppleDecoder {
    fn name(&self) -> &'static str {
        "apple-imageio"
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn probe(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
        let source = image_source(format, bytes)?;
        read_header(&source, format, bytes)
    }

    fn decode(
        &self,
        format: ImageFormat,
        bytes: &[u8],
        request: &DecodeRequest,
    ) -> Result<DecodeResponse, ImageError> {
        let source = image_source(format, bytes)?;
        let header = read_header(&source, format, bytes)?;
        request.check(&header)?;

        // `natural_size` and therefore `target` are in *oriented* space, but
        // `ImageIO`'s thumbnail cap is applied to stored pixels — so the swap has
        // to be undone before asking for a size and re-applied after.
        let target = request.effective_size(header.natural_size);
        let orientation = orientation_of(format, bytes);
        let stored_natural = unswap(orientation, header.natural_size);
        let stored_target = unswap(orientation, target);

        let image = if stored_target == stored_natural {
            // SAFETY: no options dictionary is passed, so there are no
            // dictionary generics to get wrong.
            unsafe { source.image_at_index(0, None) }.ok_or_else(|| {
                ImageError::decode(format, "ImageIO produced no image for frame 0")
            })?
        } else {
            scaled_image(&source, format, stored_natural, stored_target)?
        };

        let (pixels, width, height) = draw_rgba8(&image, format)?;
        // Orientation first, so the target size is interpreted in the same
        // space the natural size was reported in — the software backend does
        // the same, in the same order.
        let (pixels, width, height) = orientation.apply(pixels, width, height);

        // `ImageIO` preserves the source aspect ratio, so a target that does not
        // still needs the shared resampler. `scaled_image` deliberately asks for
        // a thumbnail no *smaller* than the target on either axis, so this pass
        // only ever downsamples.
        let (pixels, width, height) = if (width, height) == (target.width, target.height) {
            (pixels, width, height)
        } else {
            let scaled = resample(
                pixels,
                (width, height),
                (target.width, target.height),
                AlphaType::Premultiplied,
                format,
            )?;
            (scaled, target.width, target.height)
        };

        Ok(DecodeResponse {
            image: DecodedImage::from_rgba8(
                width,
                height,
                // The bitmap context below is created with
                // `kCGImageAlphaPremultipliedLast`, so these bytes are already
                // scaled by alpha. Carried, not converted.
                AlphaType::Premultiplied,
                pixels,
                format,
            )?,
            header,
            acceleration: Acceleration::PlatformSoftware,
            backend: self.name(),
        })
    }
}

// -------------------------------------------------------------- probe

/// The uniform type identifiers `ImageIO` uses for the three containers this
/// crate decodes.
///
/// `public.webp` is accepted alongside `org.webmproject.webp` purely
/// defensively — the WebM-project identifier is the one macOS 11 / iOS 14
/// registered, and matching both costs one comparison.
const PNG_TYPE: &str = "public.png";
const JPEG_TYPE: &str = "public.jpeg";
const WEBP_TYPES: [&str; 2] = ["org.webmproject.webp", "public.webp"];

/// Which formats `ImageIO` offers *on this machine*, memoised process-wide.
///
/// `CGImageSourceCopyTypeIdentifiers` is the whole OS-version check. `ImageIO`
/// gained WebP in macOS 11 / iOS 14 and the identifier list simply does not
/// contain it before then, so there is no version comparison, no weak linking
/// and no `#[cfg]` — which is also why the three formats are reported
/// independently rather than as one hardcoded set.
fn probe_capabilities() -> Capabilities {
    static CAPABILITIES: OnceLock<Capabilities> = OnceLock::new();

    probe_once(&CAPABILITIES, || {
        // SAFETY: `CGImageSourceCopyTypeIdentifiers` takes no arguments and is
        // documented to return a non-null `CFArray` of `CFString` UTIs.
        let identifiers = unsafe { CGImageSource::type_identifiers() };
        // SAFETY: as above — the array's elements are `CFString`, so giving the
        // untyped `CFArray` that element type is sound.
        let identifiers = unsafe { CFRetained::cast_unchecked::<CFArray<CFString>>(identifiers) };

        let mut capabilities = Capabilities::none();
        for identifier in identifiers.to_vec() {
            let identifier = identifier.to_string();
            let format = if identifier == PNG_TYPE {
                ImageFormat::Png
            } else if identifier == JPEG_TYPE {
                ImageFormat::Jpeg
            } else if WEBP_TYPES.contains(&identifier.as_str()) {
                ImageFormat::WebP
            } else {
                continue;
            };
            capabilities = capabilities.with(format, Acceleration::PlatformSoftware);
        }
        capabilities
    })
}

// ------------------------------------------------------------- header

/// Wraps the encoded bytes in a `CGImageSource`.
///
/// `CFData::from_bytes` copies. The no-copy constructor needs a `'static`
/// buffer, and the encoded bytes here are borrowed for the length of one call —
/// paying one copy of the *compressed* data is far cheaper than the peak
/// full-size bitmap the scaled decode below avoids.
fn image_source(
    format: ImageFormat,
    bytes: &[u8],
) -> Result<CFRetained<CGImageSource>, ImageError> {
    let data = CFData::from_bytes(bytes);
    // SAFETY: no options dictionary is passed, so there are no dictionary
    // generics to get wrong.
    unsafe { CGImageSource::with_data(&data, None) }
        .ok_or_else(|| ImageError::decode(format, "ImageIO could not open the byte stream"))
}

/// Header-only: `CGImageSourceCopyPropertiesAtIndex` parses container metadata
/// and never touches pixel data, so no `CGImage` is created here.
fn read_header(
    source: &CGImageSource,
    format: ImageFormat,
    bytes: &[u8],
) -> Result<ImageHeader, ImageError> {
    // SAFETY: no options dictionary is passed, so there are no dictionary
    // generics to get wrong.
    let properties = unsafe { source.properties_at_index(0, None) }
        .ok_or_else(|| ImageError::decode(format, "ImageIO returned no image properties"))?;
    // SAFETY: `CGImageSourceCopyPropertiesAtIndex` is documented to return a
    // dictionary keyed by `CFString` whose values are Core Foundation objects.
    let properties =
        unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(properties) };

    // SAFETY: reading immutable `CFString` constants exported by `ImageIO`.
    let (width_key, height_key, alpha_key) = unsafe {
        (
            kCGImagePropertyPixelWidth,
            kCGImagePropertyPixelHeight,
            kCGImagePropertyHasAlpha,
        )
    };
    let width = property_u32(&properties, width_key)
        .ok_or_else(|| ImageError::decode(format, "headers carry no pixel width"))?;
    let height = property_u32(&properties, height_key)
        .ok_or_else(|| ImageError::decode(format, "headers carry no pixel height"))?;
    if width == 0 || height == 0 {
        return Err(ImageError::decode(
            format,
            format!("headers report a zero-area image ({width}x{height})"),
        ));
    }

    let (width, height) = orientation_of(format, bytes).apply_to_size(width, height);
    // SAFETY: takes no options; reads the already-parsed container index.
    let frames = unsafe { source.count() };

    Ok(ImageHeader {
        format,
        natural_size: PixelSize { width, height },
        has_alpha: property_bool(&properties, alpha_key).unwrap_or(false),
        // An animated WebP or APNG presents more than one frame at the source
        // level; v1 decodes frame 0 and reports the rest exists.
        animated: frames > 1,
    })
}

fn property_u32(properties: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u32> {
    let value = properties.get(key)?.downcast::<CFNumber>().ok()?.as_i64()?;
    u32::try_from(value).ok()
}

fn property_bool(properties: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<bool> {
    Some(properties.get(key)?.downcast::<CFBoolean>().ok()?.as_bool())
}

/// The EXIF orientation to apply, read through the crate's own parser.
///
/// `CGImageSourceCreateImageAtIndex` never applies the tag, and the thumbnail
/// path is not given `kCGImageSourceCreateThumbnailWithTransform`, so neither
/// call orients — this backend therefore owns the transform outright and there
/// is nothing to double-apply.
///
/// `ImageIO`'s own `kCGImagePropertyOrientation` is deliberately *not* the source
/// of truth here. It is reported for PNG `eXIf` and WebP `EXIF` chunks too,
/// which the software backend does not read (recorded crate limit 8), so
/// trusting it would make the two backends disagree about a file's natural size
/// — the one thing this whole function exists to prevent. Sharing
/// [`orientation::jpeg_orientation`] makes agreement structural rather than
/// coincidental.
fn orientation_of(format: ImageFormat, bytes: &[u8]) -> Orientation {
    if format == ImageFormat::Jpeg {
        orientation::jpeg_orientation(bytes)
    } else {
        Orientation::Identity
    }
}

/// Converts a size from oriented space back into the stored space `ImageIO`
/// speaks.
const fn unswap(orientation: Orientation, size: PixelSize) -> PixelSize {
    if orientation.swaps_axes() {
        PixelSize {
            width: size.height,
            height: size.width,
        }
    } else {
        size
    }
}

// ------------------------------------------------------------- decode

/// The decode-time downsample: `CGImageSourceCreateThumbnailAtIndex` with a
/// max-pixel cap.
///
/// `kCGImageSourceCreateThumbnailFromImageAlways` forces the thumbnail to be
/// built from the full image rather than from whatever low-resolution preview a
/// camera happened to embed — an embedded thumbnail is a different picture, not
/// a smaller one.
fn scaled_image(
    source: &CGImageSource,
    format: ImageFormat,
    natural: PixelSize,
    target: PixelSize,
) -> Result<CFRetained<CGImage>, ImageError> {
    let max_pixel_size = CFNumber::new_i64(i64::from(thumbnail_cap(natural, target)));
    // SAFETY: reading immutable `CFString` constants exported by `ImageIO`.
    let (always_key, size_key) = unsafe {
        (
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceThumbnailMaxPixelSize,
        )
    };
    let options = CFDictionary::<CFString, CFType>::from_slices(
        &[always_key, size_key],
        &[CFBoolean::new(true).as_ref(), max_pixel_size.as_ref()],
    );

    // SAFETY: the options dictionary is keyed by `CFString` and holds Core
    // Foundation values, which is what `CGImageSourceCreateThumbnailAtIndex`
    // requires; both keys take the value types passed above.
    unsafe { source.thumbnail_at_index(0, Some(AsRef::as_ref(&*options))) }
        .ok_or_else(|| ImageError::decode(format, "ImageIO produced no scaled image"))
}

/// The `kCGImageSourceThumbnailMaxPixelSize` value to ask for.
///
/// `ImageIO` caps the *longest* side and preserves the aspect ratio, so a target
/// that does not preserve it cannot be hit exactly. The cap is therefore chosen
/// so the thumbnail is at least as large as the target on **both** axes, leaving
/// the final fit to [`resample`] — which downsamples. Picking the smaller cap
/// instead would make that final pass an upscale, which is visibly worse.
fn thumbnail_cap(natural: PixelSize, target: PixelSize) -> u32 {
    debug_assert!(
        natural.width > 0 && natural.height > 0,
        "read_header rejects a zero-area source before this runs"
    );
    let longest = u64::from(natural.width.max(natural.height));
    let for_width = (longest * u64::from(target.width)).div_ceil(u64::from(natural.width));
    let for_height = (longest * u64::from(target.height)).div_ceil(u64::from(natural.height));
    let cap = for_width.max(for_height).clamp(1, longest);
    u32::try_from(cap).unwrap_or(u32::MAX)
}

/// Redraws a `CGImage` into a tightly packed premultiplied RGBA8 buffer.
///
/// This pass is not overhead to be optimised away: it is what normalises
/// grayscale, palette, CMYK, 16-bit and float sources — all of which `ImageIO`
/// hands back verbatim — into the single layout the rest of the crate and
/// vello's atlas accept.
fn draw_rgba8(image: &CGImage, format: ImageFormat) -> Result<(Vec<u8>, u32, u32), ImageError> {
    let stored_width = CGImage::width(Some(image));
    let stored_height = CGImage::height(Some(image));
    let width = u32::try_from(stored_width)
        .map_err(|_| ImageError::decode(format, "decoded width does not fit u32"))?;
    let height = u32::try_from(stored_height)
        .map_err(|_| ImageError::decode(format, "decoded height does not fit u32"))?;
    if width == 0 || height == 0 {
        return Err(ImageError::decode(
            format,
            format!("ImageIO produced a zero-area image ({width}x{height})"),
        ));
    }

    let stride = stored_width
        .checked_mul(4)
        .ok_or_else(|| ImageError::too_large(width, height, "width * 4 overflows usize"))?;
    let length = expected_byte_len(width, height).ok_or_else(|| {
        ImageError::too_large(width, height, "width * height * 4 overflows usize")
    })?;
    let mut pixels = vec![0u8; length];

    // SAFETY: reading an immutable `CFString` constant exported by Core Graphics.
    let srgb = unsafe { kCGColorSpaceSRGB };
    let color_space = CGColorSpace::with_name(Some(srgb))
        .ok_or_else(|| ImageError::decode(format, "sRGB colour space unavailable"))?;
    // Premultiplied, 32-bit-big-endian RGBA: R, G, B, A in memory order, which
    // is exactly `DecodedImage`'s contract.
    let bitmap_info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;

    {
        // SAFETY: `pixels` is a live, uniquely borrowed allocation of exactly
        // `height * stride` bytes, which is what the width/height/stride
        // arguments describe. The context borrows it and is dropped at the end
        // of this block, before `pixels` is read or moved, so the pointer never
        // outlives the buffer and nothing else aliases it meanwhile.
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast::<c_void>(),
                stored_width,
                stored_height,
                8,
                stride,
                Some(&color_space),
                bitmap_info,
            )
        }
        .ok_or_else(|| ImageError::decode(format, "could not create an RGBA8 bitmap context"))?;

        // Drawn at exactly the image's own size, so no filtering happens here —
        // scaling is either `ImageIO`'s job above or the resampler's below.
        let rect = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(f64::from(width), f64::from(height)),
        );
        CGContext::draw_image(Some(&context), rect, Some(image));
    }

    Ok((pixels, width, height))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::AppleDecoder;
    use crate::backend::software::SoftwareDecoder;
    use crate::decode::{DecodeRequest, Decoder, PixelSize};
    use crate::format::ImageFormat;
    use crate::pixels::AlphaType;
    use crate::registry::Acceleration;

    /// An opaque RGBA8 PNG, encoded in-process so no fixture has to be
    /// committed.
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG header");
            let pixels: Vec<u8> = (0..width * height)
                .flat_map(|index| {
                    let value = u8::try_from(index % 251).unwrap_or(0);
                    [value, 0x20, 0x80, 0xFF]
                })
                .collect();
            writer.write_image_data(&pixels).expect("PNG image data");
        }
        out
    }

    fn apple() -> AppleDecoder {
        AppleDecoder::detect().expect("ImageIO is present on every supported macOS/iOS")
    }

    #[test]
    fn the_probe_claims_at_least_png_and_jpeg() {
        let decoder = apple();
        let capabilities = decoder.capabilities();
        for format in [ImageFormat::Png, ImageFormat::Jpeg] {
            assert_eq!(
                capabilities.tier(format),
                Some(Acceleration::PlatformSoftware),
                "{format} is an inbox ImageIO codec on every supported OS version"
            );
        }
        // WebP is version-gated (macOS 11 / iOS 14) and therefore deliberately
        // *not* asserted: the probe reports it per format, from the runtime
        // codec list, rather than hardcoding all three.
        assert_eq!(decoder.name(), "apple-imageio");
    }

    #[test]
    fn probe_reports_the_same_natural_size_as_the_software_backend() {
        let bytes = png_bytes(13, 7);
        let platform = apple()
            .probe(ImageFormat::Png, &bytes)
            .expect("ImageIO probes a well-formed PNG");
        let software = SoftwareDecoder::new()
            .probe(ImageFormat::Png, &bytes)
            .expect("the software backend probes a well-formed PNG");

        assert_eq!(platform.natural_size, software.natural_size);
        assert_eq!(
            platform.natural_size,
            PixelSize {
                width: 13,
                height: 7
            }
        );
        assert_eq!(platform.animated, software.animated);
        assert_eq!(platform.format, software.format);
    }

    #[test]
    fn a_full_size_decode_agrees_with_the_software_backend() {
        let bytes = png_bytes(16, 9);
        let request = DecodeRequest::default();
        let platform = apple()
            .decode(ImageFormat::Png, &bytes, &request)
            .expect("ImageIO decodes a well-formed PNG");
        let software = SoftwareDecoder::new()
            .decode(ImageFormat::Png, &bytes, &request)
            .expect("the software backend decodes a well-formed PNG");

        assert_eq!(platform.image.width(), software.image.width());
        assert_eq!(platform.image.height(), software.image.height());
        assert_eq!(platform.header.natural_size, software.header.natural_size);
        assert_eq!(platform.image.byte_len(), software.image.byte_len());
        // Alpha encoding is carried, not normalised — the bitmap context
        // premultiplies where the `png` crate does not.
        assert_eq!(platform.image.alpha_type(), AlphaType::Premultiplied);
        assert_eq!(platform.acceleration, Acceleration::PlatformSoftware);
        assert_eq!(platform.backend, "apple-imageio");
        // The fixture is fully opaque, so premultiplication is a no-op and the
        // two backends must produce identical bytes.
        assert_eq!(platform.image.pixels(), software.image.pixels());
    }

    #[test]
    fn a_downsample_target_is_honoured_exactly() {
        let bytes = png_bytes(32, 16);
        let request = DecodeRequest {
            target_size: Some(PixelSize {
                width: 8,
                height: 5,
            }),
            ..DecodeRequest::default()
        };
        let response = apple()
            .decode(ImageFormat::Png, &bytes, &request)
            .expect("ImageIO decodes a well-formed PNG");

        // Not aspect-preserving, so `ImageIO` gets it close and the shared
        // resampler finishes the job.
        assert_eq!(response.image.width(), 8);
        assert_eq!(response.image.height(), 5);
        // The header still reports the *source* size, not the decoded size.
        assert_eq!(
            response.header.natural_size,
            PixelSize {
                width: 32,
                height: 16
            }
        );
    }

    #[test]
    fn an_oversized_target_clamps_instead_of_upscaling() {
        let bytes = png_bytes(6, 4);
        let request = DecodeRequest {
            target_size: Some(PixelSize {
                width: 600,
                height: 400,
            }),
            ..DecodeRequest::default()
        };
        let response = apple()
            .decode(ImageFormat::Png, &bytes, &request)
            .expect("ImageIO decodes a well-formed PNG");
        assert_eq!((response.image.width(), response.image.height()), (6, 4));
    }

    #[test]
    fn a_cap_breach_is_rejected_before_any_decode() {
        let bytes = png_bytes(8, 8);
        let request = DecodeRequest {
            max_pixels: 4,
            ..DecodeRequest::default()
        };
        let error = apple()
            .decode(ImageFormat::Png, &bytes, &request)
            .expect_err("64 pixels is past a 4-pixel cap");
        assert!(format!("{error}").contains("max_pixels"));
    }

    #[test]
    fn unreadable_bytes_are_an_error_rather_than_a_panic() {
        let error = apple()
            .probe(ImageFormat::Png, b"not a PNG at all")
            .expect_err("ImageIO cannot read arbitrary bytes");
        assert!(format!("{error}").contains("PNG"));
    }
}
