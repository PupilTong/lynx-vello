//! The Apple decoder: `ImageIO`, on macOS and iOS.
//!
//! `ImageIO` is reached for two concrete reasons. First,
//! `CGImageSourceCreateThumbnailAtIndex` scales *during* decode, which for JPEG
//! means the IDCT runs at 1/2, 1/4 or 1/8 scale in the DCT domain instead of
//! decoding a full-size bitmap only to throw most of it away. Second, it is the
//! system's own codec set, so the formats Apple platforms actually traffic in —
//! HEIC from every iPhone camera, AVIF from the modern web — decode with no
//! bundled codec at all. Everything else in this file exists to make `ImageIO`'s
//! output land in the single RGBA8 shape the contract demands.
//!
//! # No runtime probe
//!
//! This workspace assumes an OS recent enough to carry every claimed codec
//! (WebP arrived in macOS 11 / iOS 14, AVIF in macOS 13 / iOS 16; the minimum
//! deployment target sits above both), so [`AppleDecoder::new`] claims all six
//! formats unconditionally and there is nothing to detect. This is a recorded
//! decision, not an oversight: the capability probe the WIC decoder still
//! carries answers a question — "is the codec installed on *this* machine?" —
//! that has no analogue on Apple platforms at the supported OS floor.
//!
//! # Recorded costs (measured 2026-08-10, Apple Silicon, 2048² photo-like fixtures)
//!
//! Measured with a since-deleted one-off divan harness comparing `ImageIO`
//! entry points against the pure-Rust codecs on identical in-process-encoded
//! bytes; the conclusions are recorded here because the choices they justify
//! are in this file. PNG through `ImageIO` measures ~30% slower than the `png`
//! crate the Linux reference uses (~51 ms vs ~39 ms) — the accepted price of
//! shipping no bundled codec on this platform. JPEG is at parity with
//! `zune-jpeg` at full size and ~2x faster at a 1/8 target, where the
//! thumbnail path runs the IDCT at reduced scale.
//! `kCGImageSourceShouldCacheImmediately` is deliberately never set: with an
//! immediate redraw into a caller buffer it measured ~2x total cost for
//! PNG/HEIC/AVIF (decode into `ImageIO`'s cache *and* the blit) and no gain
//! for JPEG/GIF. The thumbnail machinery itself costs ≤ ~15% over the plain
//! path at natural size, which is why it is only engaged when a downsample
//! target actually shrinks something. AVIF is the expensive format (~97 ms
//! full-size vs HEIC ~44 ms, JPEG ~16 ms), so decode-time downsampling
//! matters most there.
//!
//! # Why [`Acceleration::PlatformSoftware`] and never `DedicatedHardware`
//!
//! No still-image API on this platform reaches a decode ASIC and none of them
//! exposes an acceleration query. `ImageIO`'s JPEG path imports `vImage` and no
//! `IOKit` symbols — it is Apple's own vendor-tuned CPU codec, which is exactly
//! what [`Acceleration::PlatformSoftware`] means. There is no observable signal
//! that would justify claiming the reserved
//! [`DedicatedHardware`](Acceleration::DedicatedHardware) rung, so this decoder
//! never reports it.
//!
//! # Orientation
//!
//! JPEG orientation is read through [`crate::orientation`]'s own EXIF parser —
//! the same bytes-level parser the Linux reference decoder consults — so the
//! two report identical natural sizes for identical files. HEIC and AVIF store
//! EXIF inside a `meta` box that parser cannot see, and no reference decoder
//! exists for those formats to agree with, so their orientation is read from
//! `ImageIO`'s own `kCGImagePropertyOrientation`. Neither
//! `CGImageSourceCreateImageAtIndex` nor the un-transformed thumbnail path
//! applies the tag, so this decoder owns the transform outright and there is
//! nothing to double-apply.
//!
//! # Thread safety
//!
//! `CGImageSource`, `CGImage` and `CGContext` are thread-safe Core Foundation
//! types with no main-thread requirement, and this decoder never leans on that
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
//! 3. `CFRetained::cast_unchecked`, to give the untyped `CFDictionary` that `ImageIO` returns the
//!    element types its documentation promises.
//! 4. `CGBitmapContextCreate`, which borrows a caller-owned pixel buffer for the lifetime of the
//!    context.
//!
//! There is no raw pointer arithmetic, no manual retain/release (`CFRetained`
//! owns every handle) and no transmute.
#![allow(unsafe_code)]

use std::ffi::c_void;

use bobcat_core::image::{
    Acceleration, AlphaType, Capabilities, DecodeRequest, DecodeResponse, DecodedImage, Decoder,
    ImageError, ImageFormat, ImageHeader, PixelSize, expected_byte_len,
};
use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_image_io::{
    CGImageSource, kCGImagePropertyHasAlpha, kCGImagePropertyOrientation,
    kCGImagePropertyPixelHeight, kCGImagePropertyPixelWidth,
    kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceThumbnailMaxPixelSize,
};

use crate::orientation::{self, Orientation};
use crate::resample;

/// PNG, JPEG, WebP, GIF, HEIC and AVIF through `ImageIO`.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppleDecoder;

impl AppleDecoder {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Every identified format, claimed unconditionally — see the module docs for
/// why there is no probe.
const CAPABILITIES: Capabilities = Capabilities::none()
    .with(ImageFormat::Png, Acceleration::PlatformSoftware)
    .with(ImageFormat::Jpeg, Acceleration::PlatformSoftware)
    .with(ImageFormat::WebP, Acceleration::PlatformSoftware)
    .with(ImageFormat::Gif, Acceleration::PlatformSoftware)
    .with(ImageFormat::Heic, Acceleration::PlatformSoftware)
    .with(ImageFormat::Avif, Acceleration::PlatformSoftware);

impl Decoder for AppleDecoder {
    fn name(&self) -> &'static str {
        "apple-imageio"
    }

    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
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
        let orientation = orientation_of(&source, format, bytes);
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
        // space the natural size was reported in.
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
            backend: "apple-imageio",
        })
    }
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

/// Frame 0's property dictionary. Header-only:
/// `CGImageSourceCopyPropertiesAtIndex` parses container metadata and never
/// touches pixel data, so no `CGImage` is created here.
fn frame_properties(
    source: &CGImageSource,
    format: ImageFormat,
) -> Result<CFRetained<CFDictionary<CFString, CFType>>, ImageError> {
    // SAFETY: no options dictionary is passed, so there are no dictionary
    // generics to get wrong.
    let properties = unsafe { source.properties_at_index(0, None) }
        .ok_or_else(|| ImageError::decode(format, "ImageIO returned no image properties"))?;
    // SAFETY: `CGImageSourceCopyPropertiesAtIndex` is documented to return a
    // dictionary keyed by `CFString` whose values are Core Foundation objects.
    Ok(unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(properties) })
}

fn read_header(
    source: &CGImageSource,
    format: ImageFormat,
    bytes: &[u8],
) -> Result<ImageHeader, ImageError> {
    let properties = frame_properties(source, format)?;

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

    let (width, height) = orientation_of(source, format, bytes).apply_to_size(width, height);
    // SAFETY: takes no options; reads the already-parsed container index.
    let frames = unsafe { source.count() };

    Ok(ImageHeader {
        format,
        natural_size: PixelSize { width, height },
        has_alpha: property_bool(&properties, alpha_key).unwrap_or(false),
        // An animated container presents more than one frame at the source
        // level; v1 decodes frame 0 and reports the rest exists. The container
        // check catches what the count misses: an APNG whose animation has a
        // single frame still carries its `acTL`, but `ImageIO` counts 1.
        animated: frames > 1 || crate::animation::container_declares_animation(format, bytes),
    })
}

fn property_u32(properties: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u32> {
    let value = properties.get(key)?.downcast::<CFNumber>().ok()?.as_i64()?;
    u32::try_from(value).ok()
}

fn property_bool(properties: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<bool> {
    Some(properties.get(key)?.downcast::<CFBoolean>().ok()?.as_bool())
}

/// The EXIF orientation to apply. See the module docs for why JPEG reads the
/// bytes and HEIC/AVIF read `ImageIO`'s property.
fn orientation_of(source: &CGImageSource, format: ImageFormat, bytes: &[u8]) -> Orientation {
    match format {
        ImageFormat::Jpeg => orientation::jpeg_orientation(bytes),
        ImageFormat::Heic | ImageFormat::Avif => property_orientation(source, format),
        _ => Orientation::Identity,
    }
}

/// `kCGImagePropertyOrientation` for frame 0, as the transform it asks for.
///
/// The property carries the same 1..=8 vocabulary as EXIF tag `0x0112`, and a
/// missing or unreadable value reads as identity — the same policy every
/// mainstream decoder applies to a corrupt tag.
fn property_orientation(source: &CGImageSource, format: ImageFormat) -> Orientation {
    let Ok(properties) = frame_properties(source, format) else {
        return Orientation::Identity;
    };
    // SAFETY: reading an immutable `CFString` constant exported by `ImageIO`.
    let key = unsafe { kCGImagePropertyOrientation };
    let Some(value) = properties
        .get(key)
        .and_then(|value| value.downcast::<CFNumber>().ok())
        .and_then(|number| number.as_i64())
    else {
        return Orientation::Identity;
    };
    u16::try_from(value).map_or(Orientation::Identity, Orientation::from_exif)
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
/// grayscale, palette, CMYK, 16-bit, float and 10-bit HEIC/AVIF sources — all
/// of which `ImageIO` hands back verbatim — into the single layout the rest of
/// the pipeline and vello's atlas accept.
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

// ------------------------------------------------------- test support

/// Encodes RGBA8 pixels through `CGImageDestination`, so GIF/HEIC/AVIF
/// fixtures never need committing.
///
/// `None` when the running OS has no encoder for `uti` — the *decode*
/// assumption this crate makes does not extend to encoders, and a test that
/// needs one probes through this return value.
#[cfg(test)]
fn encode_rgba(width: u32, height: u32, rgba: &[u8], uti: &str) -> Option<Vec<u8>> {
    use objc2_core_foundation::CFMutableData;
    use objc2_core_graphics::CGBitmapContextCreateImage;
    use objc2_image_io::CGImageDestination;

    assert_eq!(
        rgba.len(),
        expected_byte_len(width, height).expect("fixture size overflows"),
        "encode_rgba takes tightly packed RGBA8"
    );

    // Draw the caller's pixels into a bitmap context and lift a `CGImage` out —
    // the decoder's normalisation path, run in reverse.
    let mut pixels = rgba.to_vec();
    // SAFETY: reading an immutable `CFString` constant exported by Core Graphics.
    let srgb = unsafe { kCGColorSpaceSRGB };
    let color_space = CGColorSpace::with_name(Some(srgb))?;
    let bitmap_info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    let image = {
        // SAFETY: `pixels` is a live, uniquely borrowed allocation of exactly
        // `height * (width * 4)` bytes, and the context is dropped inside this
        // block — `CGBitmapContextCreateImage` copies the bitmap.
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast::<c_void>(),
                width as usize,
                height as usize,
                8,
                width as usize * 4,
                Some(&color_space),
                bitmap_info,
            )
        }?;
        CGBitmapContextCreateImage(Some(&context))?
    };

    let data = CFMutableData::new(None, 0)?;
    let uti = CFString::from_str(uti);
    // SAFETY: the destination appends encoded bytes to `data`, whose retained
    // handle outlives it; no options dictionary is passed here or in
    // `add_image`, so there are no dictionary generics to get wrong.
    let destination = unsafe { CGImageDestination::with_data(&data, &uti, 1, None) }?;
    // SAFETY: `add_image` takes no options dictionary here, and `finalize`
    // writes the pending image into `data` — both operate on the live retained
    // destination and objects it retains itself.
    unsafe {
        CGImageDestination::add_image(&destination, &image, None);
        if !CGImageDestination::finalize(&destination) {
            return None;
        }
    }
    Some(data.to_vec())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use bobcat_core::image::{
        Acceleration, AlphaType, DecodeRequest, Decoder, ImageFormat, PixelSize, sniff,
    };

    use super::{AppleDecoder, encode_rgba};

    /// An opaque RGBA8 test pattern: a deterministic per-pixel ramp.
    fn ramp_rgba(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|index| {
                let value = u8::try_from(index % 251).unwrap_or(0);
                [value, 0x20, 0x80, 0xFF]
            })
            .collect()
    }

    /// An opaque PNG, encoded in-process with the `png` crate so PNG tests do
    /// not depend on the encoder under test.
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG header");
            writer
                .write_image_data(&ramp_rgba(width, height))
                .expect("PNG image data");
        }
        out
    }

    #[test]
    fn all_six_formats_are_claimed_unconditionally() {
        let decoder = AppleDecoder::new();
        for format in ImageFormat::ALL {
            assert_eq!(
                decoder.capabilities().tier(format),
                Some(Acceleration::PlatformSoftware),
                "{format} must be claimed — the no-probe decision is recorded in the module docs"
            );
        }
        assert_eq!(decoder.name(), "apple-imageio");
    }

    #[test]
    fn a_png_round_trips_byte_for_byte() {
        // The fixture is fully opaque, so premultiplication is a no-op and the
        // decode must reproduce the source bytes exactly — PNG is lossless.
        let source = ramp_rgba(16, 9);
        let response = AppleDecoder::new()
            .decode(
                ImageFormat::Png,
                &png_bytes(16, 9),
                &DecodeRequest::default(),
            )
            .expect("ImageIO decodes a well-formed PNG");

        assert_eq!((response.image.width(), response.image.height()), (16, 9));
        assert_eq!(response.image.pixels(), source.as_slice());
        assert_eq!(response.image.alpha_type(), AlphaType::Premultiplied);
        assert_eq!(response.acceleration, Acceleration::PlatformSoftware);
        assert_eq!(response.backend, "apple-imageio");
    }

    #[test]
    fn probing_reports_the_size_without_decoding() {
        let header = AppleDecoder::new()
            .probe(ImageFormat::Png, &png_bytes(13, 7))
            .expect("ImageIO probes a well-formed PNG");
        assert_eq!(
            header.natural_size,
            PixelSize {
                width: 13,
                height: 7
            }
        );
        assert!(!header.animated);
        assert_eq!(header.format, ImageFormat::Png);
    }

    #[test]
    fn a_downsample_target_is_honoured_exactly() {
        let request = DecodeRequest::default().with_target(Some(PixelSize {
            width: 8,
            height: 5,
        }));
        let response = AppleDecoder::new()
            .decode(ImageFormat::Png, &png_bytes(32, 16), &request)
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
        let request = DecodeRequest::default().with_target(Some(PixelSize {
            width: 600,
            height: 400,
        }));
        let response = AppleDecoder::new()
            .decode(ImageFormat::Png, &png_bytes(6, 4), &request)
            .expect("ImageIO decodes a well-formed PNG");
        assert_eq!((response.image.width(), response.image.height()), (6, 4));
    }

    #[test]
    fn a_cap_breach_is_rejected_before_any_decode() {
        let request = DecodeRequest::default().with_max_pixels(4);
        let error = AppleDecoder::new()
            .decode(ImageFormat::Png, &png_bytes(8, 8), &request)
            .expect_err("64 pixels is past a 4-pixel cap");
        assert!(format!("{error}").contains("max_pixels"));
    }

    #[test]
    fn unreadable_bytes_are_an_error_rather_than_a_panic() {
        let error = AppleDecoder::new()
            .probe(ImageFormat::Png, b"not a PNG at all")
            .expect_err("ImageIO cannot read arbitrary bytes");
        assert!(format!("{error}").contains("PNG"));
    }

    /// The formats the reference decoder never sees: encode through `ImageIO`,
    /// sniff the result to prove the contract identifies it, then decode it
    /// back and check dimensions and content survive.
    #[test]
    fn gif_heic_and_avif_round_trip_through_imageio() {
        let decoder = AppleDecoder::new();
        // A flat colour rather than the ramp: GIF quantises to 256 colours and
        // HEIC/AVIF default to lossy, so content is asserted with a tolerance.
        let source: Vec<u8> = std::iter::repeat_n([200u8, 60, 30, 255], 12 * 8)
            .flatten()
            .collect();

        for (uti, format) in [
            ("com.compuserve.gif", ImageFormat::Gif),
            ("public.heic", ImageFormat::Heic),
            ("public.avif", ImageFormat::Avif),
        ] {
            let Some(bytes) = encode_rgba(12, 8, &source, uti) else {
                panic!("this host's ImageIO offers no {uti} encoder — see encode_rgba's docs");
            };
            assert_eq!(sniff(&bytes), Some(format), "{uti} must sniff as {format}");

            let header = decoder
                .probe(format, &bytes)
                .unwrap_or_else(|error| panic!("probe {format}: {error}"));
            assert_eq!(
                header.natural_size,
                PixelSize {
                    width: 12,
                    height: 8
                }
            );
            assert!(!header.animated, "a single-frame {format} is still");

            let response = decoder
                .decode(format, &bytes, &DecodeRequest::default())
                .unwrap_or_else(|error| panic!("decode {format}: {error}"));
            assert_eq!((response.image.width(), response.image.height()), (12, 8));
            let [r, g, b, a]: [u8; 4] = response.image.pixels()[0..4].try_into().expect("a pixel");
            assert!(
                r.abs_diff(200) < 24 && g.abs_diff(60) < 24 && b.abs_diff(30) < 24 && a == 255,
                "{format} decode drifted too far: got {r},{g},{b},{a}"
            );
        }
    }
}
