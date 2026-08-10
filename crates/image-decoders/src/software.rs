//! The Linux-only pure-Rust reference decoder.
//!
//! Linux is the one supported OS with no system still-image decode API, and
//! headless CI runs there — that is this decoder's entire constituency. Every
//! other target ships its platform decoder alone, so this module is compiled
//! for `target_os = "linux"` and nowhere else.
//!
//! One crate per format rather than the `image` facade: these are the same three
//! decoders `image` 0.25 delegates to, reached directly so each one's own memory
//! limit is actually wired up and so RGBA8 comes out without the facade's
//! intermediate conversion.
//!
//! Decoding always runs at full resolution here — none of the three exposes a
//! scaled decode — so a downsample target costs peak full-size memory and a
//! resampling pass. That is a memory profile difference from the platform
//! decoders, not a behavioural one.

use std::io::Cursor;

use bobcat_core::image::{
    Acceleration, AlphaType, Capabilities, DecodeRequest, DecodeResponse, DecodedImage, Decoder,
    ImageError, ImageFormat, ImageHeader, PixelSize,
};

use crate::orientation::{self, Orientation};
use crate::resample;

/// What each per-format decoder hands back: the RGBA8 buffer, its **stored**
/// dimensions (pre-orientation), and how it encodes alpha.
type RawDecode = (Vec<u8>, (u32, u32), AlphaType);

/// The three claimed formats. GIF, HEIC and AVIF are identified by the contract
/// but deliberately unclaimed here: each would cost another bundled codec, and
/// the platforms where those formats matter decode them through the system.
const CAPABILITIES: Capabilities = Capabilities::none()
    .with(ImageFormat::Png, Acceleration::Software)
    .with(ImageFormat::Jpeg, Acceleration::Software)
    .with(ImageFormat::WebP, Acceleration::Software);

/// PNG, JPEG and WebP via `png`, `zune-jpeg` and `image-webp`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SoftwareDecoder;

impl SoftwareDecoder {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Decoder for SoftwareDecoder {
    fn name(&self) -> &'static str {
        "software"
    }

    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    fn probe(&self, format: ImageFormat, bytes: &[u8]) -> Result<ImageHeader, ImageError> {
        match format {
            ImageFormat::Png => probe_png(bytes),
            ImageFormat::Jpeg => probe_jpeg(bytes),
            ImageFormat::WebP => probe_webp(bytes),
            // Unreachable through `decode_bytes`/the loader, which gate on
            // `capabilities` first; a direct caller gets the same refusal.
            _ => Err(ImageError::Unsupported { format }),
        }
    }

    fn decode(
        &self,
        format: ImageFormat,
        bytes: &[u8],
        request: &DecodeRequest,
    ) -> Result<DecodeResponse, ImageError> {
        let header = self.probe(format, bytes)?;
        request.check(&header)?;

        let (pixels, stored_size, alpha_type) = match format {
            ImageFormat::Png => decode_png(bytes)?,
            ImageFormat::Jpeg => decode_jpeg(bytes)?,
            ImageFormat::WebP => decode_webp(bytes, request)?,
            _ => return Err(ImageError::Unsupported { format }),
        };

        // Orientation is applied before resampling so the target size is
        // interpreted in the same space the natural size is reported in.
        let orientation = if format == ImageFormat::Jpeg {
            orientation::jpeg_orientation(bytes)
        } else {
            Orientation::Identity
        };
        let (pixels, width, height) = orientation.apply(pixels, stored_size.0, stored_size.1);

        let target = request.effective_size(header.natural_size);
        let (pixels, width, height) = if (width, height) == (target.width, target.height) {
            (pixels, width, height)
        } else {
            let scaled = resample(
                pixels,
                (width, height),
                (target.width, target.height),
                alpha_type,
                format,
            )?;
            (scaled, target.width, target.height)
        };

        Ok(DecodeResponse {
            image: DecodedImage::from_rgba8(width, height, alpha_type, pixels, format)?,
            header,
            acceleration: Acceleration::Software,
            backend: "software",
        })
    }
}

// ---------------------------------------------------------------- PNG

/// `read_info` stops after the header chunks, before any `IDAT` is inflated, so
/// this is genuinely header-only.
fn png_reader(bytes: &[u8]) -> Result<png::Reader<Cursor<&[u8]>>, ImageError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // EXPAND + STRIP_16: palette and sub-byte grayscale become 8-bit channels,
    // `tRNS` becomes a real alpha channel, and 16-bit samples are truncated to
    // 8. What survives is one of four 8-bit layouts, normalised below.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    decoder
        .read_info()
        .map_err(|error| ImageError::decode(ImageFormat::Png, error.to_string()))
}

fn probe_png(bytes: &[u8]) -> Result<ImageHeader, ImageError> {
    let reader = png_reader(bytes)?;
    let info = reader.info();
    Ok(ImageHeader {
        format: ImageFormat::Png,
        natural_size: PixelSize {
            width: info.width,
            height: info.height,
        },
        has_alpha: info.color_type.samples() % 2 == 0 || info.trns.is_some(),
        animated: info.animation_control.is_some(),
    })
}

fn decode_png(bytes: &[u8]) -> Result<RawDecode, ImageError> {
    let mut reader = png_reader(bytes)?;
    let canvas = {
        let info = reader.info();
        (info.width, info.height)
    };

    // APNG's default image is only animation frame 0 when an `fcTL` precedes
    // `IDAT`. When `acTL` comes after it, the default image is a *fallback* for
    // non-APNG decoders and is not part of the animation at all — returning it
    // would quietly break this crate's documented "decode frame 0" policy and,
    // for the common "transparent fallback" authoring pattern, hand back an
    // empty image. `Info::frame_control` is `Some` after `read_info` exactly
    // when the default image is frame 0, which is precisely the test needed.
    let default_image_is_frame_zero = reader.info().frame_control.is_some();
    let separate_fallback =
        reader.info().animation_control.is_some() && !default_image_is_frame_zero;
    if separate_fallback {
        // Skip the fallback and advance to the real first animation frame.
        reader
            .next_frame_info()
            .map_err(|error| ImageError::decode(ImageFormat::Png, error.to_string()))?;
    }

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| ImageError::decode(ImageFormat::Png, "output buffer size overflows"))?;
    let mut buffer = vec![0u8; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| ImageError::decode(ImageFormat::Png, error.to_string()))?;
    buffer.truncate(info.buffer_size());

    let (color_type, _) = reader.output_color_type();
    let rgba = expand_to_rgba(buffer, color_type, info.width, info.height)?;

    // An animation frame may be a sub-rectangle of the canvas, so it has to be
    // composited at its offset rather than returned as if it were the whole
    // image. Frame 0 alone needs no blend-mode handling: the canvas starts
    // fully transparent, and `Over` onto transparent black is `Source`, so both
    // APNG blend ops agree here. Later frames would need real dispose/blend
    // state, which static-only v1 does not keep.
    if separate_fallback {
        let frame = reader.info().frame_control.ok_or_else(|| {
            ImageError::decode(ImageFormat::Png, "animation frame carries no fcTL")
        })?;
        let composited = composite_onto_canvas(
            &rgba,
            (info.width, info.height),
            canvas,
            (frame.x_offset, frame.y_offset),
        )?;
        return Ok((composited, canvas, AlphaType::Straight));
    }

    // PNG alpha is stored straight, and `png` never premultiplies.
    Ok((rgba, (info.width, info.height), AlphaType::Straight))
}

/// Places an RGBA8 sub-rectangle onto a transparent canvas at `offset`.
fn composite_onto_canvas(
    frame: &[u8],
    frame_size: (u32, u32),
    canvas: (u32, u32),
    offset: (u32, u32),
) -> Result<Vec<u8>, ImageError> {
    let (frame_width, frame_height) = frame_size;
    let (canvas_width, canvas_height) = canvas;
    if offset.0 + frame_width > canvas_width || offset.1 + frame_height > canvas_height {
        return Err(ImageError::decode(
            ImageFormat::Png,
            format!(
                "animation frame {frame_width}x{frame_height} at ({}, {}) escapes the \
                 {canvas_width}x{canvas_height} canvas",
                offset.0, offset.1
            ),
        ));
    }
    let stride = canvas_width as usize * 4;
    let mut out = vec![0u8; stride * canvas_height as usize];
    for row in 0..frame_height as usize {
        let from = row * frame_width as usize * 4;
        let to = (row + offset.1 as usize) * stride + offset.0 as usize * 4;
        let width = frame_width as usize * 4;
        out[to..to + width].copy_from_slice(&frame[from..from + width]);
    }
    Ok(out)
}

/// Normalises `png`'s four possible 8-bit output layouts to RGBA8.
///
/// Takes the buffer by value so the already-RGBA case — by far the most common
/// — is a truncation rather than a full copy of the image. At the 8192x8192
/// ceiling that copy was a quarter of a gigabyte of pure waste.
fn expand_to_rgba(
    mut buffer: Vec<u8>,
    color_type: png::ColorType,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, ImageError> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ImageError::too_large(width, height, "width * height overflows usize"))?;
    let channels = color_type.samples();
    if buffer.len() < pixels * channels {
        return Err(ImageError::decode(
            ImageFormat::Png,
            format!(
                "decoded {} bytes, expected {} for {width}x{height} with {channels} channel(s)",
                buffer.len(),
                pixels * channels
            ),
        ));
    }
    if color_type == png::ColorType::Rgba {
        buffer.truncate(pixels * 4);
        return Ok(buffer);
    }

    let mut rgba = Vec::with_capacity(pixels * 4);
    for index in 0..pixels {
        let source = &buffer[index * channels..index * channels + channels];
        match color_type {
            png::ColorType::Grayscale => {
                rgba.extend_from_slice(&[source[0], source[0], source[0], 255]);
            }
            png::ColorType::GrayscaleAlpha => {
                rgba.extend_from_slice(&[source[0], source[0], source[0], source[1]]);
            }
            png::ColorType::Rgb => {
                rgba.extend_from_slice(&[source[0], source[1], source[2], 255]);
            }
            png::ColorType::Rgba => unreachable!("handled above"),
            // `EXPAND` converts palette to RGB/RGBA, so this cannot survive.
            png::ColorType::Indexed => {
                return Err(ImageError::decode(
                    ImageFormat::Png,
                    "indexed colour survived EXPAND",
                ));
            }
        }
    }
    Ok(rgba)
}

// --------------------------------------------------------------- JPEG

fn probe_jpeg(bytes: &[u8]) -> Result<ImageHeader, ImageError> {
    let mut decoder = zune_jpeg::JpegDecoder::new(Cursor::new(bytes));
    decoder
        .decode_headers()
        .map_err(|error| ImageError::decode(ImageFormat::Jpeg, error.to_string()))?;
    let (width, height) = jpeg_dimensions(&decoder)?;
    let (width, height) = orientation::jpeg_orientation(bytes).apply_to_size(width, height);
    Ok(ImageHeader {
        format: ImageFormat::Jpeg,
        natural_size: PixelSize { width, height },
        // Baseline and progressive JPEG have no alpha channel.
        has_alpha: false,
        animated: false,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Result<RawDecode, ImageError> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(
        Cursor::new(bytes),
        DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA),
    );
    let pixels = decoder
        .decode()
        .map_err(|error| ImageError::decode(ImageFormat::Jpeg, error.to_string()))?;
    let (width, height) = jpeg_dimensions(&decoder)?;
    // The synthesised alpha channel is fully opaque, so straight and
    // premultiplied are the same bytes; report straight.
    Ok((pixels, (width, height), AlphaType::Straight))
}

/// `zune-jpeg` reports `usize` dimensions; everything above speaks `u32`.
fn jpeg_dimensions<R>(decoder: &zune_jpeg::JpegDecoder<R>) -> Result<(u32, u32), ImageError>
where
    R: zune_jpeg::zune_core::bytestream::ZByteReaderTrait,
{
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| ImageError::decode(ImageFormat::Jpeg, "headers carry no dimensions"))?;
    let width = u32::try_from(width)
        .map_err(|_| ImageError::decode(ImageFormat::Jpeg, "width does not fit u32"))?;
    let height = u32::try_from(height)
        .map_err(|_| ImageError::decode(ImageFormat::Jpeg, "height does not fit u32"))?;
    Ok((width, height))
}

// --------------------------------------------------------------- WebP

fn webp_decoder(bytes: &[u8]) -> Result<image_webp::WebPDecoder<Cursor<&[u8]>>, ImageError> {
    image_webp::WebPDecoder::new(Cursor::new(bytes))
        .map_err(|error| ImageError::decode(ImageFormat::WebP, error.to_string()))
}

fn probe_webp(bytes: &[u8]) -> Result<ImageHeader, ImageError> {
    let decoder = webp_decoder(bytes)?;
    let (width, height) = decoder.dimensions();
    Ok(ImageHeader {
        format: ImageFormat::WebP,
        natural_size: PixelSize { width, height },
        has_alpha: decoder.has_alpha(),
        animated: decoder.is_animated(),
    })
}

fn decode_webp(bytes: &[u8], request: &DecodeRequest) -> Result<RawDecode, ImageError> {
    let mut decoder = webp_decoder(bytes)?;
    // Unlike the other two, `image-webp` takes a real memory ceiling; give it
    // the same budget the caller's pixel cap implies.
    decoder.set_memory_limit(
        usize::try_from(request.max_pixels.saturating_mul(4)).unwrap_or(usize::MAX),
    );
    let (width, height) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();
    let size = decoder
        .output_buffer_size()
        .ok_or_else(|| ImageError::decode(ImageFormat::WebP, "output buffer size overflows"))?;
    let mut buffer = vec![0u8; size];
    // An animated WebP reads its first frame here; the rest is discarded.
    decoder
        .read_image(&mut buffer)
        .map_err(|error| ImageError::decode(ImageFormat::WebP, error.to_string()))?;

    // `image-webp` writes RGB8 for an opaque image and RGBA8 otherwise.
    let rgba = if has_alpha {
        buffer
    } else {
        let pixels = (width as usize) * (height as usize);
        let mut rgba = Vec::with_capacity(pixels * 4);
        for index in 0..pixels {
            let source = &buffer[index * 3..index * 3 + 3];
            rgba.extend_from_slice(&[source[0], source[1], source[2], 255]);
        }
        rgba
    };
    Ok((rgba, (width, height), AlphaType::Straight))
}
