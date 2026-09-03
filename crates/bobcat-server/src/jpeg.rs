//! Conversion from Bobcat's tightly packed RGBA8 readback to the JPEG served
//! by the screenshot endpoint.

use std::io::Cursor;

use bobcat_core::Screenshot;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;

const JPEG_QUALITY: u8 = 90;

#[derive(Debug, thiserror::Error)]
pub(crate) enum JpegError {
    #[error("a {width}\u{d7}{height} RGBA frame needs {expected} bytes, got {actual}")]
    BufferSize {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    #[error("failed to encode the screenshot as JPEG: {0}")]
    Encode(#[from] image::ImageError),
}

/// JPEG has no alpha channel, so readback is composited over white before it
/// leaves the process. That is the ordinary visual presentation of a partly
/// transparent page and matches UI Judge's screenshot response contract.
pub(crate) fn encode(screenshot: &Screenshot) -> Result<Vec<u8>, JpegError> {
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
        return Err(JpegError::BufferSize {
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
    JpegEncoder::new_with_quality(&mut output, JPEG_QUALITY).encode(
        &rgb,
        width,
        height,
        ExtendedColorType::Rgb8,
    )?;
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
        let jpeg = encode(&screenshot([20, 40, 60, 255])).expect("encode JPEG");
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);

        let decoded = image::load_from_memory(&jpeg)
            .expect("decode JPEG")
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (8, 8));
        let pixel = decoded.get_pixel(4, 4);
        assert!(
            pixel[0].abs_diff(20) < 8 && pixel[1].abs_diff(40) < 8 && pixel[2].abs_diff(60) < 8,
            "unexpected color after encoding: {pixel:?}"
        );
    }

    #[test]
    fn composites_transparent_pixels_over_white() {
        let jpeg = encode(&screenshot([0, 0, 0, 0])).expect("encode JPEG");
        let decoded = image::load_from_memory(&jpeg)
            .expect("decode JPEG")
            .to_rgb8();
        assert!(
            decoded
                .get_pixel(4, 4)
                .0
                .iter()
                .all(|channel| *channel > 245)
        );
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
