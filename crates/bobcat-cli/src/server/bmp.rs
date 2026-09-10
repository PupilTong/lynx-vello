// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License, Version 2.0.
//! UI Judge's lossless BMP layout, adapted from headless-rust-test-runner/src/bmp.rs.

use bobcat_core::Screenshot;

#[derive(Debug, thiserror::Error)]
#[error("failed to encode screenshot as BMP: {0}")]
pub(crate) struct BmpError(String);

const FILE_HEADER_LEN: u32 = 14;
const INFO_HEADER_LEN: u32 = 108;
const PIXEL_OFFSET: u32 = FILE_HEADER_LEN + INFO_HEADER_LEN;
const BI_BITFIELDS: u32 = 3;
const LCS_S_RGB: u32 = 0x7352_4742;
const PIXELS_PER_METER: i32 = 2835;

/// Encodes RGBA pixels as a top-down 32-bit BMP with an explicit alpha mask.
///
/// A `BITMAPV4HEADER` with `BI_BITFIELDS` is the only widely decodable BMP
/// variant that preserves alpha: readers drop the fourth channel of a plain
/// `BI_RGB` 32-bit bitmap.
pub(crate) fn encode(screenshot: &Screenshot) -> Result<Vec<u8>, BmpError> {
    let width = screenshot.size.width as usize;
    let height = screenshot.size.height as usize;
    let rgba = &screenshot.pixels;
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| BmpError("frame is too large".into()))?;
    let expected = pixels
        .checked_mul(4)
        .ok_or_else(|| BmpError("frame is too large".into()))?;
    if rgba.len() != expected {
        return Err(BmpError(
            "frame buffer size does not match its dimensions".into(),
        ));
    }
    let file_len = (PIXEL_OFFSET as usize)
        .checked_add(expected)
        .ok_or_else(|| BmpError("frame is too large".into()))?;
    let width = i32::try_from(width)
        .map_err(|_| BmpError(format!("frame width {width} exceeds the BMP limit")))?;
    let height = i32::try_from(height)
        .map_err(|_| BmpError(format!("frame height {height} exceeds the BMP limit")))?;
    let file_len_field = u32::try_from(file_len)
        .map_err(|_| BmpError(format!("frame of {file_len} bytes exceeds the BMP limit")))?;

    let mut output = Vec::with_capacity(file_len);
    output.extend_from_slice(b"BM");
    output.extend_from_slice(&file_len_field.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&PIXEL_OFFSET.to_le_bytes());

    output.extend_from_slice(&INFO_HEADER_LEN.to_le_bytes());
    output.extend_from_slice(&width.to_le_bytes());
    // A negative height marks the rows as top-down, matching the presented frame.
    output.extend_from_slice(&(-height).to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&32_u16.to_le_bytes());
    output.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
    output.extend_from_slice(&(file_len_field - PIXEL_OFFSET).to_le_bytes());
    output.extend_from_slice(&PIXELS_PER_METER.to_le_bytes());
    output.extend_from_slice(&PIXELS_PER_METER.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0x00FF_0000_u32.to_le_bytes());
    output.extend_from_slice(&0x0000_FF00_u32.to_le_bytes());
    output.extend_from_slice(&0x0000_00FF_u32.to_le_bytes());
    output.extend_from_slice(&0xFF00_0000_u32.to_le_bytes());
    output.extend_from_slice(&LCS_S_RGB.to_le_bytes());
    output.extend_from_slice(&[0_u8; 36]);
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());

    for pixel in rgba[..expected].chunks_exact(4) {
        output.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use bobcat_core::{FrameSize, Screenshot};

    use super::*;

    #[test]
    fn matches_ui_judges_top_down_alpha_preserving_layout() {
        let frame = Screenshot {
            size: FrameSize {
                width: 2,
                height: 2,
            },
            pixels: vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 1, 2, 3, 0],
        };
        let bmp = encode(&frame).unwrap();
        let field = |offset| u32::from_le_bytes(bmp[offset..offset + 4].try_into().unwrap());
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(field(2), 138);
        assert_eq!(field(6), 0);
        assert_eq!(field(10), 122);
        assert_eq!(field(14), 108);
        assert_eq!(field(18), 2);
        assert_eq!(i32::from_le_bytes(bmp[22..26].try_into().unwrap()), -2);
        assert_eq!(&bmp[26..30], &[1, 0, 32, 0]);
        assert_eq!(field(30), 3);
        assert_eq!(field(34), 16);
        assert_eq!(field(38), 2835);
        assert_eq!(field(42), 2835);
        assert_eq!(field(46), 0);
        assert_eq!(field(50), 0);
        assert_eq!(field(54), 0x00ff_0000);
        assert_eq!(field(58), 0x0000_ff00);
        assert_eq!(field(62), 0x0000_00ff);
        assert_eq!(field(66), 0xff00_0000);
        assert_eq!(field(70), 0x7352_4742);
        assert!(bmp[74..122].iter().all(|byte| *byte == 0));
        assert_eq!(
            &bmp[122..],
            &[0, 0, 255, 255, 0, 255, 0, 128, 255, 0, 0, 255, 3, 2, 1, 0]
        );
        let decoded = image::load_from_memory(&bmp).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(decoded.into_raw(), frame.pixels);
    }

    #[test]
    fn odd_width_rows_have_no_padding_and_keep_transparency() {
        let frame = Screenshot {
            size: FrameSize {
                width: 1,
                height: 2,
            },
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 128],
        };
        let bmp = encode(&frame).unwrap();
        assert_eq!(bmp.len(), 130);
        assert_eq!(
            image::load_from_memory(&bmp).unwrap().to_rgba8().into_raw(),
            frame.pixels
        );
    }

    #[test]
    fn rejects_malformed_or_oversized_frames() {
        for (width, height, pixels) in [
            (2, 1, vec![0; 7]),
            (2, 1, vec![0; 9]),
            (u32::MAX, u32::MAX, Vec::new()),
        ] {
            assert!(
                encode(&Screenshot {
                    size: FrameSize { width, height },
                    pixels
                })
                .is_err()
            );
        }
    }
}
