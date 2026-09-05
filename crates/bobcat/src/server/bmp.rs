//! Conversion from Bobcat's tightly packed RGBA8 readback to the BMP served
//! by the screenshot endpoint.

use std::io::Cursor;

use bobcat_core::Screenshot;
use image::ExtendedColorType;
use image::codecs::bmp::BmpEncoder;

#[derive(Debug, thiserror::Error)]
pub(crate) enum BmpError {
    #[error("a {width}\u{d7}{height} RGBA frame needs {expected} bytes, got {actual}")]
    BufferSize {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    #[error("failed to encode the screenshot as BMP: {0}")]
    Encode(#[from] image::ImageError),
}

/// Emit uncompressed 24-bit BMP, keeping the endpoint's white background by
/// compositing the RGBA readback before encoding.
pub(crate) fn encode(screenshot: &Screenshot) -> Result<Vec<u8>, BmpError> {
    let width = screenshot.size.width;
    let height = screenshot.size.height;
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(usize::MAX);
    if screenshot.pixels.len() != expected {
        return Err(BmpError::BufferSize {
            width,
            height,
            expected,
            actual: screenshot.pixels.len(),
        });
    }

    let mut rgb = Vec::with_capacity(expected / 4 * 3);
    for pixel in screenshot.pixels.chunks_exact(4) {
        let alpha = u16::from(pixel[3]);
        for channel in &pixel[..3] {
            // Rounded integer source-over composition onto opaque white.
            let value = u16::from(*channel) * alpha + 255 * (255 - alpha);
            rgb.push(
                u8::try_from((value + 127) / 255)
                    .expect("an opaque RGB channel must remain within eight bits"),
            );
        }
    }

    let mut output = Cursor::new(Vec::new());
    BmpEncoder::new(&mut output).encode(&rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use bobcat_core::{FrameSize, Screenshot};

    use super::encode;

    fn screenshot(color: [u8; 4]) -> Screenshot {
        Screenshot {
            size: FrameSize {
                width: 8,
                height: 8,
            },
            pixels: color.repeat(64),
        }
    }

    #[test]
    fn encodes_an_opaque_frame_and_keeps_its_color() {
        let bmp = encode(&screenshot([20, 40, 60, 255])).expect("encode BMP");
        assert_eq!(&bmp[..2], b"BM");

        let decoded = image::load_from_memory(&bmp).expect("decode BMP").to_rgb8();
        assert_eq!(decoded.dimensions(), (8, 8));
        let pixel = decoded.get_pixel(4, 4);
        assert_eq!(pixel.0, [20, 40, 60]);
    }

    #[test]
    fn composites_transparent_pixels_over_white() {
        let bmp = encode(&screenshot([0, 0, 0, 0])).expect("encode BMP");
        let decoded = image::load_from_memory(&bmp).expect("decode BMP").to_rgb8();
        assert_eq!(decoded.get_pixel(4, 4).0, [255, 255, 255]);
    }

    #[test]
    fn preserves_row_order_padding_and_partial_alpha() {
        let frame = Screenshot {
            size: FrameSize {
                width: 1,
                height: 2,
            },
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 128],
        };
        let bmp = encode(&frame).expect("encode padded BMP rows");
        // BITMAPINFOHEADER: 24-bit pixels, BI_RGB (no compression).
        assert_eq!(u16::from_le_bytes(bmp[28..30].try_into().unwrap()), 24);
        assert_eq!(u32::from_le_bytes(bmp[30..34].try_into().unwrap()), 0);
        let decoded = image::load_from_memory(&bmp).expect("decode BMP").to_rgb8();
        assert_eq!(decoded.dimensions(), (1, 2));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(decoded.get_pixel(0, 1).0, [127, 127, 255]);
    }

    #[test]
    fn rejects_a_malformed_readback() {
        let error = encode(&Screenshot {
            size: FrameSize {
                width: 2,
                height: 1,
            },
            pixels: vec![0; 7],
        })
        .expect_err("seven bytes cannot hold two RGBA pixels");
        assert!(error.to_string().contains("needs 8 bytes, got 7"));
    }
}
