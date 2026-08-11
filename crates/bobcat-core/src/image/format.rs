//! Container identification, done in-crate from the leading bytes.
//!
//! Sniffing is deliberately ours rather than a decoder crate's: the backends
//! are chosen *per format*, so the format has to be known before any of them is
//! constructed, and the two questions a caller actually has — "which container
//! is this?" and "did all of it arrive?" — are answered by different amounts of
//! the buffer.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
    Gif,
    Heic,
    Avif,
}

impl ImageFormat {
    pub const ALL: [Self; 6] = [
        Self::Png,
        Self::Jpeg,
        Self::WebP,
        Self::Gif,
        Self::Heic,
        Self::Avif,
    ];

    #[must_use]
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Png => 0,
            Self::Jpeg => 1,
            Self::WebP => 2,
            Self::Gif => 3,
            Self::Heic => 4,
            Self::Avif => 5,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
            Self::Gif => "GIF",
            Self::Heic => "HEIC",
            Self::Avif => "AVIF",
        }
    }

    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Heic => "image/heic",
            Self::Avif => "image/avif",
        }
    }
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Identifies the container from its magic bytes.
#[must_use]
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&PNG_MAGIC) {
        return Some(ImageFormat::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return sniff_bmff_brands(bytes);
    }
    None
}

const HEIC_BRANDS: [&[u8; 4]; 10] = [
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1", b"msf1",
];
const AVIF_BRANDS: [&[u8; 4]; 2] = [b"avif", b"avis"];

fn sniff_bmff_brands(bytes: &[u8]) -> Option<ImageFormat> {
    let brand_matches =
        |brand: &[u8], set: &[&[u8; 4]]| set.iter().any(|entry| &entry[..] == brand);

    let major = &bytes[8..12];
    if brand_matches(major, &AVIF_BRANDS) {
        return Some(ImageFormat::Avif);
    }
    if brand_matches(major, &HEIC_BRANDS[..8]) {
        return Some(ImageFormat::Heic);
    }
    if !brand_matches(major, &HEIC_BRANDS) {
        return None;
    }

    let box_len = usize::try_from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .unwrap_or(usize::MAX)
        .min(bytes.len());
    let mut cursor = 16;
    while cursor + 4 <= box_len {
        if brand_matches(&bytes[cursor..cursor + 4], &AVIF_BRANDS) {
            return Some(ImageFormat::Avif);
        }
        cursor += 4;
    }
    Some(ImageFormat::Heic)
}

/// Whether the container's own framing accounts for every byte it declares.
#[must_use]
pub fn is_complete(format: ImageFormat, bytes: &[u8]) -> bool {
    match format {
        ImageFormat::Png => has_png_iend(bytes),
        ImageFormat::Jpeg => has_jpeg_eoi(bytes),
        ImageFormat::WebP => webp_payload_present(bytes),
        ImageFormat::Gif => has_gif_trailer(bytes),
        ImageFormat::Heic | ImageFormat::Avif => bmff_boxes_complete(bytes),
    }
}

fn has_png_iend(bytes: &[u8]) -> bool {
    let mut cursor = PNG_MAGIC.len();
    loop {
        let Some(header) = bytes.get(cursor..cursor + 8) else {
            return false;
        };
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = &header[4..8];
        if length > i32::MAX as u32 {
            return false;
        }
        let Some(end) = (cursor + 8)
            .checked_add(length as usize)
            .and_then(|payload_end| payload_end.checked_add(4))
        else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if kind == b"IEND" {
            return length == 0;
        }
        cursor = end;
    }
}

fn has_jpeg_eoi(bytes: &[u8]) -> bool {
    let mut cursor = 2usize;
    loop {
        while bytes.get(cursor) == Some(&0xFF) && bytes.get(cursor + 1) == Some(&0xFF) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&0xFF) {
            return false;
        }
        let Some(&marker) = bytes.get(cursor + 1) else {
            return false;
        };
        match marker {
            0xD9 => return true,
            0x01 | 0xD0..=0xD7 => cursor += 2,
            0xDA => {
                let Some(length) = segment_length(bytes, cursor) else {
                    return false;
                };
                cursor += 2 + length;
                let Some(next) = scan_to_next_marker(bytes, cursor) else {
                    return false;
                };
                cursor = next;
            }
            _ => {
                let Some(length) = segment_length(bytes, cursor) else {
                    return false;
                };
                cursor += 2 + length;
            }
        }
    }
}

fn segment_length(bytes: &[u8], at: usize) -> Option<usize> {
    let field = bytes.get(at + 2..at + 4)?;
    let length = usize::from(u16::from_be_bytes([field[0], field[1]]));
    if length < 2 || at + 2 + length > bytes.len() {
        return None;
    }
    Some(length)
}

fn scan_to_next_marker(bytes: &[u8], from: usize) -> Option<usize> {
    let mut cursor = from;
    loop {
        cursor += bytes.get(cursor..)?.iter().position(|byte| *byte == 0xFF)?;
        let &next = bytes.get(cursor + 1)?;
        match next {
            0xFF => cursor += 1,
            0x00 | 0xD0..=0xD7 => cursor += 2,
            _ => return Some(cursor),
        }
    }
}

fn has_gif_trailer(bytes: &[u8]) -> bool {
    let Some(descriptor) = bytes.get(6..13) else {
        return false;
    };
    let flags = descriptor[4];
    let mut cursor = 13usize;
    if flags & 0x80 != 0 {
        cursor += 3usize << ((flags & 0x07) + 1);
    }

    loop {
        match bytes.get(cursor) {
            Some(0x3B) => return true,
            Some(0x21) => {
                let Some(next) = skip_gif_subblocks(bytes, cursor + 2) else {
                    return false;
                };
                cursor = next;
            }
            Some(0x2C) => {
                let Some(&local_flags) = bytes.get(cursor + 9) else {
                    return false;
                };
                let mut data = cursor + 10;
                if local_flags & 0x80 != 0 {
                    data += 3usize << ((local_flags & 0x07) + 1);
                }
                let Some(next) = skip_gif_subblocks(bytes, data + 1) else {
                    return false;
                };
                cursor = next;
            }
            _ => return false,
        }
    }
}

fn skip_gif_subblocks(bytes: &[u8], from: usize) -> Option<usize> {
    let mut cursor = from;
    loop {
        let &length = bytes.get(cursor)?;
        cursor += 1 + usize::from(length);
        if length == 0 {
            return Some(cursor);
        }
    }
}

fn bmff_boxes_complete(bytes: &[u8]) -> bool {
    let mut cursor = 0u64;
    let len = bytes.len() as u64;
    while cursor < len {
        let Some(header) = usize::try_from(cursor)
            .ok()
            .and_then(|at| bytes.get(at..at + 8))
        else {
            return false;
        };
        let declared = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let size = match declared {
            0 => return true,
            1 => {
                let Some(large) = usize::try_from(cursor)
                    .ok()
                    .and_then(|at| bytes.get(at + 8..at + 16))
                else {
                    return false;
                };
                let size = u64::from_be_bytes([
                    large[0], large[1], large[2], large[3], large[4], large[5], large[6], large[7],
                ]);
                if size < 16 {
                    return false;
                }
                size
            }
            2..8 => return false,
            _ => declared,
        };
        let Some(next) = cursor.checked_add(size) else {
            return false;
        };
        if next > len {
            return false;
        }
        cursor = next;
    }
    true
}

fn webp_payload_present(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(4..8) else {
        return false;
    };
    let declared = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    declared >= 4 && u64::from(declared) + 8 <= bytes.len() as u64
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{ImageFormat, is_complete, sniff};

    fn png_bytes() -> Vec<u8> {
        let mut bytes = super::PNG_MAGIC.to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0u8; 13]);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(b"IEND");
        bytes.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
        bytes
    }

    fn webp_bytes(payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        #[allow(clippy::cast_possible_truncation)]
        let declared = (payload.len() + 4) as u32;
        bytes.extend_from_slice(&declared.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(payload);
        bytes
    }

    fn ftyp(major: [u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + 4 * compatible.len();
        #[allow(clippy::cast_possible_truncation)]
        let mut bytes = (size as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ftyp");
        bytes.extend_from_slice(&major);
        bytes.extend_from_slice(&[0, 0, 0, 0]); // minor version
        for brand in compatible {
            bytes.extend_from_slice(*brand);
        }
        bytes
    }

    #[test]
    fn sniff_identifies_each_supported_container() {
        assert_eq!(sniff(&png_bytes()), Some(ImageFormat::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageFormat::Jpeg));
        assert_eq!(sniff(&webp_bytes(b"VP8 ")), Some(ImageFormat::WebP));
        assert_eq!(sniff(b"GIF89a......"), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"GIF87a......"), Some(ImageFormat::Gif));
        assert_eq!(sniff(&ftyp(*b"heic", &[])), Some(ImageFormat::Heic));
        assert_eq!(sniff(&ftyp(*b"avif", &[])), Some(ImageFormat::Avif));
    }

    #[test]
    fn sniff_rejects_containers_this_crate_does_not_identify() {
        assert_eq!(sniff(b"BM..........").map(ImageFormat::as_str), None);
        assert_eq!(sniff(&ftyp(*b"isom", &[b"mp42"])), None);
        assert_eq!(sniff(b"<svg xmlns='...'/>"), None);
    }

    #[test]
    fn a_generic_heif_brand_is_decided_by_its_compatible_brands() {
        assert_eq!(
            sniff(&ftyp(*b"mif1", &[b"avif", b"miaf"])),
            Some(ImageFormat::Avif)
        );
        assert_eq!(
            sniff(&ftyp(*b"mif1", &[b"heic", b"miaf"])),
            Some(ImageFormat::Heic)
        );
        assert_eq!(sniff(&ftyp(*b"mif1", &[])), Some(ImageFormat::Heic));
    }

    #[test]
    fn sniff_rejects_empty_and_short_buffers() {
        assert_eq!(sniff(&[]), None);
        assert_eq!(sniff(&[0x89, b'P', b'N']), None);
        assert_eq!(sniff(&[0xFF, 0xD8]), None);
        assert_eq!(sniff(b"RIFF\x04\0\0\0"), None);
    }

    #[test]
    fn sniff_ignores_the_riff_length_field() {
        let mut bytes = webp_bytes(b"VP8 ");
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(sniff(&bytes), Some(ImageFormat::WebP));
        assert!(!is_complete(ImageFormat::WebP, &bytes));
    }

    #[test]
    fn is_complete_requires_the_terminating_marker() {
        assert!(is_complete(ImageFormat::Png, &png_bytes()));
        let truncated = &png_bytes()[..16];
        assert!(!is_complete(ImageFormat::Png, truncated));

        assert!(is_complete(ImageFormat::Jpeg, &[0xFF, 0xD8, 0xFF, 0xD9]));
        assert!(!is_complete(
            ImageFormat::Jpeg,
            &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02]
        ));

        assert!(is_complete(ImageFormat::WebP, &webp_bytes(b"VP8 data")));
        let short = webp_bytes(b"VP8 data");
        assert!(!is_complete(ImageFormat::WebP, &short[..short.len() - 3]));
    }

    #[test]
    fn png_completeness_walks_chunks_rather_than_scanning_for_iend() {
        let complete = png_bytes();
        assert!(is_complete(ImageFormat::Png, &complete));

        let no_crc = &complete[..complete.len() - 4];
        assert!(
            !is_complete(ImageFormat::Png, no_crc),
            "the IEND literal without its CRC is not a complete chunk"
        );
        assert!(!is_complete(
            ImageFormat::Png,
            &complete[..complete.len() - 1]
        ));

        let mut decoy = super::PNG_MAGIC.to_vec();
        decoy.extend_from_slice(&8u32.to_be_bytes());
        decoy.extend_from_slice(b"IDAT");
        decoy.extend_from_slice(b"xxIENDyy");
        decoy.extend_from_slice(&[0u8; 4]);
        assert!(
            !is_complete(ImageFormat::Png, &decoy),
            "IEND inside a payload is data, not a terminator"
        );

        let mut both = decoy.clone();
        both.extend_from_slice(&0u32.to_be_bytes());
        both.extend_from_slice(b"IEND");
        both.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
        assert!(is_complete(ImageFormat::Png, &both));
    }

    fn jpeg_with_segment(marker: u8, payload: &[u8], tail: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, marker];
        #[allow(clippy::cast_possible_truncation)]
        let length = (payload.len() + 2) as u16;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(tail);
        bytes
    }

    #[test]
    fn jpeg_completeness_skips_segment_payloads_rather_than_scanning_them() {
        let decoy = jpeg_with_segment(0xE1, &[0xFF, 0xD9], &[]);
        assert!(
            !is_complete(ImageFormat::Jpeg, &decoy),
            "EOI inside an APP payload is not the stream's terminator"
        );

        let terminated = jpeg_with_segment(0xE1, &[0xFF, 0xD9], &[0xFF, 0xD9]);
        assert!(is_complete(ImageFormat::Jpeg, &terminated));
    }

    #[test]
    fn jpeg_completeness_walks_scan_data_byte_stuffing_aware() {
        let scan = jpeg_with_segment(
            0xDA,
            &[0x01],
            &[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xD9],
        );
        assert!(is_complete(ImageFormat::Jpeg, &scan));

        let unterminated = jpeg_with_segment(0xDA, &[0x01], &[0x12, 0xFF, 0x00, 0x34]);
        assert!(!is_complete(ImageFormat::Jpeg, &unterminated));
    }

    #[test]
    fn jpeg_completeness_rejects_a_segment_running_past_the_buffer() {
        let mut lying = vec![0xFF, 0xD8, 0xFF, 0xE0];
        lying.extend_from_slice(&9000u16.to_be_bytes());
        lying.extend_from_slice(&[0u8; 4]);
        assert!(!is_complete(ImageFormat::Jpeg, &lying));
    }

    #[test]
    fn png_completeness_rejects_a_nonsense_chunk_length() {
        let mut absurd = super::PNG_MAGIC.to_vec();
        absurd.extend_from_slice(&u32::MAX.to_be_bytes());
        absurd.extend_from_slice(b"IDAT");
        absurd.extend_from_slice(&[0u8; 8]);
        assert!(!is_complete(ImageFormat::Png, &absurd));
    }

    #[test]
    fn is_complete_tolerates_trailing_bytes_after_jpeg_eoi() {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xD9];
        bytes.extend_from_slice(&[0x00; 16]);
        assert!(is_complete(ImageFormat::Jpeg, &bytes));
    }

    fn gif_bytes(tail: &[u8]) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&[1, 0, 1, 0]); // 1x1 canvas
        bytes.push(0x80); // global colour table, size bits 0 => 2 entries
        bytes.extend_from_slice(&[0, 0]); // background, aspect
        bytes.extend_from_slice(&[0; 6]); // the 2-entry colour table
        bytes.push(0x2C); // image descriptor
        bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0, 0]); // 1x1, no LCT
        bytes.push(0x02); // LZW minimum code size
        bytes.extend_from_slice(&[2, 0x4C, 0x01]); // one 2-byte sub-block
        bytes.push(0); // sub-block terminator
        bytes.extend_from_slice(tail);
        bytes
    }

    #[test]
    fn gif_completeness_requires_the_trailer_after_walking_the_blocks() {
        assert!(is_complete(ImageFormat::Gif, &gif_bytes(&[0x3B])));

        assert!(!is_complete(ImageFormat::Gif, &gif_bytes(&[])));
        let complete = gif_bytes(&[0x3B]);
        assert!(!is_complete(ImageFormat::Gif, &complete[..20]));

        let mut decoy = b"GIF89a".to_vec();
        decoy.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]);
        decoy.extend_from_slice(&[0x3B; 6]); // colour table full of trailers
        assert!(!is_complete(ImageFormat::Gif, &decoy));
    }

    #[test]
    fn bmff_completeness_walks_top_level_boxes() {
        let mut bytes = ftyp(*b"avif", &[]);
        bytes.extend_from_slice(&12u32.to_be_bytes());
        bytes.extend_from_slice(b"mdat");
        bytes.extend_from_slice(&[0u8; 4]);
        assert!(is_complete(ImageFormat::Avif, &bytes));

        assert!(!is_complete(ImageFormat::Avif, &bytes[..bytes.len() - 2]));

        let mut open_ended = ftyp(*b"heic", &[]);
        open_ended.extend_from_slice(&0u32.to_be_bytes());
        open_ended.extend_from_slice(b"mdat");
        open_ended.extend_from_slice(&[0u8; 9]);
        assert!(is_complete(ImageFormat::Heic, &open_ended));

        let mut absurd = ftyp(*b"heic", &[]);
        absurd.extend_from_slice(&3u32.to_be_bytes());
        absurd.extend_from_slice(b"mdat");
        assert!(!is_complete(ImageFormat::Heic, &absurd));
    }
}
