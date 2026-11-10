//! Image container headers: which format a byte string is, and the size the
//! header declares for it — without decoding a pixel.
//!
//! Two consumers need this before any decoder runs. Sniffing needs the
//! format, because an image's magic bytes outrank whatever label it came
//! with. The image pipeline needs the size, because the intrinsic size is a
//! layout input the engine wants as soon as the bytes land, and because a
//! platform decoder that downsamples during decode is asked for a target
//! size it cannot compute without the source size. Everything here is a
//! fixed-offset read or a marker walk over the header; the pixel data is
//! never touched, which is what keeps this crate free of codecs.

use crate::mime::ImageFormat;

/// What a container header says about its image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageHeader {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
}

/// The container `bytes` start with, by magic, or `None` for anything this
/// crate does not recognise.
///
/// SVG is the one text container: it is recognised by an `<svg` root
/// element after any BOM, whitespace, XML declaration, comments and doctype,
/// scanned within the first few kilobytes only.
#[must_use]
pub fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::WebP)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if looks_like_svg(bytes) {
        Some(ImageFormat::Svg)
    } else {
        None
    }
}

/// Reads the dimensions the header declares, or `None` when the bytes are
/// not a container this crate knows, are truncated before the size, or
/// declare a zero axis. Never panics, whatever the input.
#[must_use]
pub fn probe(bytes: &[u8]) -> Option<ImageHeader> {
    let format = detect_format(bytes)?;
    let (width, height) = match format {
        ImageFormat::Png => probe_png(bytes)?,
        ImageFormat::Jpeg => probe_jpeg(bytes)?,
        ImageFormat::Gif => probe_gif(bytes)?,
        ImageFormat::WebP => probe_webp(bytes)?,
        ImageFormat::Bmp => probe_bmp(bytes)?,
        ImageFormat::Svg | ImageFormat::Other => return None,
    };
    if width == 0 || height == 0 {
        return None;
    }
    Some(ImageHeader {
        format,
        width,
        height,
    })
}

fn be_u16(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u32::from(u16::from_be_bytes([slice[0], slice[1]])))
}

fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u32::from(u16::from_le_bytes([slice[0], slice[1]])))
}

fn le_u24(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 3)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], 0]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// PNG: the IHDR chunk is required to come first, at a fixed offset.
fn probe_png(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let width = be_u32(bytes, 16)?;
    let height = be_u32(bytes, 20)?;
    // The PNG spec caps both at 2^31 - 1.
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return None;
    }
    Some((width, height))
}

/// JPEG: walk the marker segments to the first start-of-frame, which carries
/// the size; `APPn`, `COM` and table segments before it are skipped by length.
fn probe_jpeg(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut position = 2;
    loop {
        // Fill bytes: any run of 0xFF before a marker code.
        while *bytes.get(position)? == 0xFF {
            position += 1;
        }
        if position < 3 || bytes[position - 1] != 0xFF {
            // The byte after the previous segment is not a marker prefix: not
            // a well-formed marker stream.
            return None;
        }
        let marker = bytes[position];
        position += 1;
        match marker {
            // Standalone markers carry no length.
            0x01 | 0xD0..=0xD7 | 0xD8 => {}
            // End of image or start of scan without a frame header: no size.
            0xD9 | 0xDA => return None,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                let length = be_u16(bytes, position)?;
                if length < 7 {
                    return None;
                }
                let height = be_u16(bytes, position + 3)?;
                let width = be_u16(bytes, position + 5)?;
                return Some((width, height));
            }
            _ => {
                let length = be_u16(bytes, position)? as usize;
                if length < 2 {
                    return None;
                }
                position += length;
            }
        }
    }
}

/// GIF: the logical screen descriptor follows the six-byte signature.
fn probe_gif(bytes: &[u8]) -> Option<(u32, u32)> {
    Some((le_u16(bytes, 6)?, le_u16(bytes, 8)?))
}

/// WebP: three container flavours, each with its own size encoding.
fn probe_webp(bytes: &[u8]) -> Option<(u32, u32)> {
    match bytes.get(12..16)? {
        b"VP8 " => {
            // A key frame starts with the 3-byte start code 9D 01 2A, then
            // 14-bit width and height (the top two bits are scale flags).
            if bytes.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((le_u16(bytes, 26)? & 0x3FFF, le_u16(bytes, 28)? & 0x3FFF))
        }
        b"VP8L" => {
            if *bytes.get(20)? != 0x2F {
                return None;
            }
            let bits = le_u32(bytes, 21)?;
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => Some((le_u24(bytes, 24)? + 1, le_u24(bytes, 27)? + 1)),
        _ => None,
    }
}

/// BMP: the DIB header's own size selects the OS/2 core layout (16-bit
/// axes) or the Windows layout (signed 32-bit axes, negative for top-down).
fn probe_bmp(bytes: &[u8]) -> Option<(u32, u32)> {
    let header_size = le_u32(bytes, 14)?;
    if header_size == 12 {
        return Some((le_u16(bytes, 18)?, le_u16(bytes, 20)?));
    }
    if header_size < 40 {
        return None;
    }
    let width = le_u32(bytes, 18)?.cast_signed();
    let height = le_u32(bytes, 22)?.cast_signed();
    Some((width.unsigned_abs(), height.unsigned_abs()))
}

/// Whether the bytes open with an `<svg` root element, past a BOM, ASCII
/// whitespace, an XML declaration, comments and a doctype.
fn looks_like_svg(bytes: &[u8]) -> bool {
    const SCAN_LIMIT: usize = 4096;
    let mut rest = bytes.get(..bytes.len().min(SCAN_LIMIT)).unwrap_or(bytes);
    if let Some(stripped) = rest.strip_prefix(b"\xEF\xBB\xBF") {
        rest = stripped;
    }
    loop {
        rest = rest.trim_ascii_start();
        if let Some(after) = rest.strip_prefix(b"<?") {
            let Some(end) = find(after, b"?>") else {
                return false;
            };
            rest = &after[end + 2..];
        } else if let Some(after) = rest.strip_prefix(b"<!--") {
            let Some(end) = find(after, b"-->") else {
                return false;
            };
            rest = &after[end + 3..];
        } else if let Some(after) = rest.strip_prefix(b"<!") {
            let Some(end) = after.iter().position(|byte| *byte == b'>') else {
                return false;
            };
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    let Some(after) = rest.strip_prefix(b"<") else {
        return false;
    };
    let name_matches = after
        .get(..3)
        .is_some_and(|name| name.eq_ignore_ascii_case(b"svg"));
    name_matches
        && after
            .get(3)
            .is_none_or(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/' | b':'))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13_u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    fn jpeg_with_segments(
        prefix_segments: &[(u8, &[u8])],
        sof: u8,
        width: u16,
        height: u16,
    ) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];
        for (marker, payload) in prefix_segments {
            bytes.extend_from_slice(&[0xFF, *marker]);
            let length = u16::try_from(payload.len() + 2).expect("segment fits");
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(payload);
        }
        // Fill bytes are legal before a marker.
        bytes.extend_from_slice(&[0xFF, 0xFF, sof]);
        bytes.extend_from_slice(&17_u16.to_be_bytes());
        bytes.push(8);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.push(3);
        bytes
    }

    #[test]
    fn png_dimensions_come_from_ihdr() {
        assert_eq!(
            probe(&png(640, 480)),
            Some(ImageHeader {
                format: ImageFormat::Png,
                width: 640,
                height: 480
            })
        );
        assert_eq!(probe(&png(0, 480)), None, "a zero axis is no size");
        assert_eq!(
            probe(&png(640, 480)[..20]),
            None,
            "truncated before the height"
        );
        let mut wrong_chunk = png(1, 1);
        wrong_chunk[12..16].copy_from_slice(b"IDAT");
        assert_eq!(probe(&wrong_chunk), None);
        assert_eq!(probe(&png(1 << 31, 1)), None, "past the PNG bound");
    }

    #[test]
    fn jpeg_dimensions_come_from_the_first_frame_header_past_app_segments() {
        let exif = [b'E', b'x', b'i', b'f', 0, 0, 1, 2, 3];
        let comment = b"a comment";
        let bytes = jpeg_with_segments(
            &[(0xE1, &exif), (0xFE, comment), (0xDB, &[0; 4])],
            0xC2,
            1024,
            768,
        );
        assert_eq!(
            probe(&bytes),
            Some(ImageHeader {
                format: ImageFormat::Jpeg,
                width: 1024,
                height: 768
            })
        );
        assert_eq!(
            probe(&bytes[..bytes.len() - 6]),
            None,
            "truncated inside the frame header"
        );
        // A start-of-scan before any frame header means no size is coming.
        let mut sos_first = vec![0xFF, 0xD8, 0xFF, 0xDA, 0, 2];
        sos_first.extend_from_slice(&[0; 8]);
        assert_eq!(probe(&sos_first), None);
        // A restart marker is standalone and skipped without a length.
        let mut with_restart = vec![0xFF, 0xD8, 0xFF, 0xD0];
        with_restart.extend_from_slice(&jpeg_with_segments(&[], 0xC0, 7, 9)[2..]);
        assert_eq!(
            probe(&with_restart).map(|header| (header.width, header.height)),
            Some((7, 9))
        );
        // A segment length that lies about itself cannot walk anywhere.
        assert_eq!(probe(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x01]), None);
        assert_eq!(probe(&[0xFF, 0xD8, 0xFF]), None);
    }

    #[test]
    fn gif_dimensions_come_from_the_logical_screen() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&300_u16.to_le_bytes());
        bytes.extend_from_slice(&200_u16.to_le_bytes());
        assert_eq!(
            probe(&bytes),
            Some(ImageHeader {
                format: ImageFormat::Gif,
                width: 300,
                height: 200
            })
        );
        assert_eq!(probe(b"GIF87a\x2c\x01"), None);
    }

    fn webp(chunk: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        bytes.extend_from_slice(&chunk);
        bytes.extend_from_slice(&u32::try_from(payload.len()).expect("small").to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn webp_dimensions_come_from_each_container_flavour() {
        let mut lossy = vec![0; 3];
        lossy.extend_from_slice(&[0x9D, 0x01, 0x2A]);
        lossy.extend_from_slice(&(0x0226_u16 | 0x4000).to_le_bytes());
        lossy.extend_from_slice(&368_u16.to_le_bytes());
        assert_eq!(
            probe(&webp(*b"VP8 ", &lossy)).map(|header| (header.width, header.height)),
            Some((550, 368)),
            "the top two bits of each VP8 axis are scale flags"
        );

        let bits: u32 = (1023 - 1) | ((767 - 1) << 14);
        let mut lossless = vec![0x2F];
        lossless.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(
            probe(&webp(*b"VP8L", &lossless)).map(|header| (header.width, header.height)),
            Some((1023, 767))
        );

        let mut extended = vec![0; 4];
        extended.extend_from_slice(&(4095_u32 - 1).to_le_bytes()[..3]);
        extended.extend_from_slice(&(2047_u32 - 1).to_le_bytes()[..3]);
        assert_eq!(
            probe(&webp(*b"VP8X", &extended)).map(|header| (header.width, header.height)),
            Some((4095, 2047))
        );

        assert_eq!(probe(&webp(*b"VP8 ", &[0; 5])), None, "no start code");
        assert_eq!(
            probe(&webp(*b"VP8L", &[0x00, 0, 0, 0, 0])),
            None,
            "no signature byte"
        );
        assert_eq!(
            probe(&webp(*b"ALPH", &[0; 10])),
            None,
            "an unknown first chunk"
        );
        assert_eq!(
            detect_format(b"RIFF\x00\x00\x00\x00WAVE"),
            None,
            "a RIFF that is not WebP"
        );
    }

    #[test]
    fn bmp_dimensions_honour_both_dib_layouts_and_top_down_heights() {
        let mut windows = b"BM".to_vec();
        windows.extend_from_slice(&[0; 12]);
        windows.extend_from_slice(&40_u32.to_le_bytes());
        windows.extend_from_slice(&800_i32.to_le_bytes());
        windows.extend_from_slice(&(-600_i32).to_le_bytes());
        assert_eq!(
            probe(&windows),
            Some(ImageHeader {
                format: ImageFormat::Bmp,
                width: 800,
                height: 600
            })
        );

        let mut core = b"BM".to_vec();
        core.extend_from_slice(&[0; 12]);
        core.extend_from_slice(&12_u32.to_le_bytes());
        core.extend_from_slice(&32_u16.to_le_bytes());
        core.extend_from_slice(&16_u16.to_le_bytes());
        assert_eq!(
            probe(&core).map(|header| (header.width, header.height)),
            Some((32, 16))
        );

        let mut unknown = b"BM".to_vec();
        unknown.extend_from_slice(&[0; 12]);
        unknown.extend_from_slice(&20_u32.to_le_bytes());
        unknown.extend_from_slice(&[0; 8]);
        assert_eq!(probe(&unknown), None);
        assert_eq!(probe(b"BM\x00"), None);
    }

    #[test]
    fn svg_is_detected_past_prologue_noise_and_not_elsewhere() {
        assert_eq!(detect_format(b"<svg xmlns='x'/>"), Some(ImageFormat::Svg));
        assert_eq!(
            detect_format(b"\xEF\xBB\xBF \n<?xml version='1.0'?><!DOCTYPE svg><!-- x --><SVG>"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(detect_format(b"<svg:svg/>"), Some(ImageFormat::Svg));
        assert_eq!(detect_format(b"<svgfoo/>"), None);
        assert_eq!(detect_format(b"<html><svg/></html>"), None);
        assert_eq!(
            detect_format(b"<?xml version='1.0'"),
            None,
            "unterminated declaration"
        );
        assert_eq!(detect_format(b""), None);
        assert_eq!(probe(b"<svg/>"), None, "an SVG has no header size");
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        let junk: Vec<u8> = (0..=255).cycle().take(4096).collect();
        for length in 0..junk.len() {
            let _ = probe(&junk[..length]);
        }
        for prefix in [
            &b"\x89PNG\r\n\x1a\n"[..],
            b"\xFF\xD8\xFF",
            b"GIF89a",
            b"RIFF\x00\x00\x00\x00WEBP",
            b"BM",
        ] {
            let mut bytes = prefix.to_vec();
            for byte in 0..=255_u8 {
                bytes.push(byte);
                let _ = probe(&bytes);
            }
        }
    }
}
