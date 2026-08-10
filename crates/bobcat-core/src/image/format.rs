//! Container identification, done in-crate from the leading bytes.
//!
//! Sniffing is deliberately ours rather than a decoder crate's: the backends
//! are chosen *per format*, so the format has to be known before any of them is
//! constructed, and the two questions a caller actually has — "which container
//! is this?" and "did all of it arrive?" — are answered by different amounts of
//! the buffer.

use std::fmt;

/// The containers this module *identifies*. Which of them decode is the injected
/// [`Decoder`](crate::image::Decoder)'s to claim, per format, through
/// [`Capabilities`](crate::image::Capabilities) — the Linux reference decoder claims
/// the first three, the Apple decoder all six.
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
    /// Every container this module sniffs, in a stable order.
    pub const ALL: [Self; 6] = [
        Self::Png,
        Self::Jpeg,
        Self::WebP,
        Self::Gif,
        Self::Heic,
        Self::Avif,
    ];

    /// This format's position in [`Self::ALL`], for per-format tables.
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

/// Identifies the container from its magic bytes. Reads at most the first 12.
///
/// This looks only at the signatures, never at declared lengths — a WebP whose
/// RIFF size field is nonsense is still recognisably a WebP, and saying so is
/// what lets [`is_complete`] produce the specific
/// [`Truncated`](crate::image::ImageError::Truncated) error instead of a useless
/// "unrecognised container".
#[must_use]
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&PNG_MAGIC) {
        return Some(ImageFormat::Png);
    }
    // SOI + the first marker byte. Every JPEG stream, JFIF or Exif, opens this
    // way; the third byte is the first marker's own identifier.
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

/// The HEIF-family brands `ImageIO` opens as HEIC. `mif1`/`msf1` are the generic
/// structural brands a HEIF file may lead with when no codec-specific brand is
/// primary.
const HEIC_BRANDS: [&[u8; 4]; 10] = [
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1", b"msf1",
];
const AVIF_BRANDS: [&[u8; 4]; 2] = [b"avif", b"avis"];

/// Distinguishes AVIF from HEIC inside an ISO-BMFF `ftyp` box.
///
/// The major brand at 8..12 decides when it is codec-specific. A generic
/// `mif1`/`msf1` major brand decides nothing by itself, so the compatible-brand
/// list (12.. within the `ftyp` box) is scanned: an `avif`/`avis` entry there
/// marks AVIF, any HEIC-family entry marks HEIC. AVIF is checked first in the
/// scan because every AVIF file is structurally a HEIF and routinely lists
/// `mif1` alongside `avif`.
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
        // An ISO-BMFF container this module does not identify (MP4, JPEG 2000…).
        return None;
    }

    // Generic HEIF major brand: let the compatible brands decide.
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
    // A generic HEIF with no AVIF brand anywhere — including a bare `mif1` with
    // no codec brand at all — opens through the HEIF path on the platforms that
    // decode it; HEIC is the honest bucket.
    Some(ImageFormat::Heic)
}

/// Whether the container's own framing accounts for every byte it declares.
///
/// Runs before backend dispatch on every platform, because the backends
/// disagree about truncated input and the divergence must never reach one: a
/// PNG cut mid-`IDAT` decodes through `ImageIO` to a full-size, entirely
/// transparent image that reports itself complete, while the software decoder
/// errors.
///
/// This is framing only, not integrity — it does not verify CRCs or entropy
/// data, so a corrupt-but-fully-framed file still reaches the decoder and fails
/// there as [`Decode`](crate::image::ImageError::Decode).
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

/// A PNG is complete once a well-formed, zero-length `IEND` chunk — **including
/// its four CRC bytes** — has been reached by walking the chunk chain.
///
/// Scanning for the literal `IEND` is not enough, and both failure directions
/// are real. A file truncated immediately after the `IEND` type but before its
/// mandatory CRC contains the bytes yet is not complete; and `IEND` is four
/// unremarkable ASCII bytes that can occur inside any other chunk's compressed
/// payload, so a scan can also report complete far too early. Walking
/// length-prefixed chunks costs one pass over the headers, touches no payload,
/// and answers exactly the question asked.
fn has_png_iend(bytes: &[u8]) -> bool {
    // Every chunk is: 4-byte big-endian payload length, 4-byte type, payload,
    // 4-byte CRC.
    let mut cursor = PNG_MAGIC.len();
    loop {
        let Some(header) = bytes.get(cursor..cursor + 8) else {
            return false;
        };
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = &header[4..8];
        // The spec caps a chunk length at 2^31 - 1; anything larger is
        // corruption rather than a chunk we have not seen yet.
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
            // `IEND` is defined to carry no payload; a non-empty one means the
            // chain is not the one the spec describes.
            return length == 0;
        }
        cursor = end;
    }
}

/// Whether a terminal `EOI` marker is reachable by walking the JPEG marker
/// chain.
///
/// Searching for a bare `FF D9` is wrong: those two bytes occur freely inside
/// an `APP` segment's payload (an embedded EXIF thumbnail is itself a JPEG, so
/// it carries its own `EOI`) and inside entropy-coded scan data. A file whose
/// only `FF D9` sits in a metadata payload would be called complete while its
/// image data is still missing.
///
/// So: length-prefixed segments are skipped by their declared size, and scan
/// data is walked byte-stuffing-aware — inside a scan, `FF 00` is a literal
/// `FF` and `FF D0..=FF D7` are restart markers, neither of which ends the
/// stream. Trailing bytes after a genuine `EOI` are tolerated, because cameras
/// and stripping tools routinely leave padding and every decoder stops there.
fn has_jpeg_eoi(bytes: &[u8]) -> bool {
    // Past SOI.
    let mut cursor = 2usize;
    loop {
        // Markers may be preceded by any number of `FF` fill bytes.
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
            // EOI: the stream is terminated.
            0xD9 => return true,
            // Standalone markers, no payload: TEM and RST0..=RST7.
            0x01 | 0xD0..=0xD7 => cursor += 2,
            // SOS: a length-prefixed header followed by entropy-coded data that
            // is not length-prefixed at all, so it has to be scanned.
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
            // Everything else is length-prefixed; skip its payload wholesale so
            // an embedded thumbnail's own `EOI` is never mistaken for ours.
            _ => {
                let Some(length) = segment_length(bytes, cursor) else {
                    return false;
                };
                cursor += 2 + length;
            }
        }
    }
}

/// The declared length of the segment whose marker starts at `at`, or `None`
/// when it is absent, degenerate, or runs past the buffer.
fn segment_length(bytes: &[u8], at: usize) -> Option<usize> {
    let field = bytes.get(at + 2..at + 4)?;
    let length = usize::from(u16::from_be_bytes([field[0], field[1]]));
    // The length counts itself, so anything under 2 is malformed.
    if length < 2 || at + 2 + length > bytes.len() {
        return None;
    }
    Some(length)
}

/// Walks entropy-coded data to the next real marker, honouring byte stuffing.
fn scan_to_next_marker(bytes: &[u8], from: usize) -> Option<usize> {
    let mut cursor = from;
    loop {
        // Find the next `FF`.
        cursor += bytes.get(cursor..)?.iter().position(|byte| *byte == 0xFF)?;
        let &next = bytes.get(cursor + 1)?;
        match next {
            // Fill byte: re-examine from the second `FF`.
            0xFF => cursor += 1,
            // A stuffed literal `FF` (`FF 00`), or a restart marker: both are
            // two bytes that belong to the scan and do not end it.
            0x00 | 0xD0..=0xD7 => cursor += 2,
            // Any other marker ends the entropy-coded segment.
            _ => return Some(cursor),
        }
    }
}

/// A GIF is complete once the trailer byte (`0x3B`) is reached by walking the
/// block chain — header, logical screen descriptor, optional global colour
/// table, then extensions and image descriptors, each of whose data rides in
/// length-prefixed sub-blocks terminated by a zero length.
///
/// Walking rather than scanning for the same reason as PNG and JPEG: `0x3B` is
/// an unremarkable byte that occurs freely inside colour tables and LZW data,
/// so a scan reports complete far too early; and a file cut mid-sub-block still
/// *contains* plenty of `0x3B` bytes while being truncated.
fn has_gif_trailer(bytes: &[u8]) -> bool {
    // 6-byte signature + 7-byte logical screen descriptor.
    let Some(descriptor) = bytes.get(6..13) else {
        return false;
    };
    // Bit 7 of the flags byte announces a global colour table of
    // `3 * 2^(size+1)` bytes, size in bits 0..=2.
    let flags = descriptor[4];
    let mut cursor = 13usize;
    if flags & 0x80 != 0 {
        cursor += 3usize << ((flags & 0x07) + 1);
    }

    loop {
        match bytes.get(cursor) {
            // Trailer: the stream is terminated.
            Some(0x3B) => return true,
            // Extension: label byte, then sub-blocks.
            Some(0x21) => {
                let Some(next) = skip_gif_subblocks(bytes, cursor + 2) else {
                    return false;
                };
                cursor = next;
            }
            // Image descriptor: 9 fixed bytes, an optional local colour table,
            // one LZW minimum-code-size byte, then sub-blocks.
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
            // An unknown block type is corruption, not a block we have not
            // seen: GIF defines no others.
            _ => return false,
        }
    }
}

/// Walks a chain of length-prefixed GIF sub-blocks starting at `from`,
/// returning the offset just past the zero-length terminator.
fn skip_gif_subblocks(bytes: &[u8], from: usize) -> Option<usize> {
    let mut cursor = from;
    loop {
        let &length = bytes.get(cursor)?;
        cursor += 1 + usize::from(length);
        if length == 0 {
            // The terminator must itself be inside the buffer, which the `get`
            // above already proved.
            return Some(cursor);
        }
        // The next length byte is validated by the `get` at the top.
    }
}

/// An ISO-BMFF file (HEIC, AVIF) is complete when every top-level box's
/// declared size is fully present and the boxes account for the whole buffer.
///
/// Framing only, like every other check here: a file truncated *exactly* at a
/// box boundary passes and fails in the decoder as
/// [`Decode`](crate::image::ImageError::Decode), the same way a fully-framed but
/// corrupt PNG does.
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
            // Size 0: the box extends to the end of the file, which by
            // definition is all present.
            0 => return true,
            // Size 1: a 64-bit largesize follows the type.
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
                // The largesize counts the 16 header bytes it follows.
                if size < 16 {
                    return false;
                }
                size
            }
            // Anything below a bare box header is malformed.
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

/// RIFF declares its payload length at bytes 4..8, little-endian, counting
/// everything after that field. The buffer must hold all of it.
fn webp_payload_present(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(4..8) else {
        return false;
    };
    let declared = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    // A zero-length or absurd declaration is framing we cannot trust.
    declared >= 4 && u64::from(declared) + 8 <= bytes.len() as u64
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{ImageFormat, is_complete, sniff};

    /// A minimal but structurally valid chunk chain: one 13-byte `IHDR`, then
    /// the zero-length `IEND` with its CRC.
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

    /// An ISO-BMFF `ftyp` box with the given major and compatible brands.
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
        // ISO-BMFF, but not a still-image brand this module names.
        assert_eq!(sniff(&ftyp(*b"isom", &[b"mp42"])), None);
        assert_eq!(sniff(b"<svg xmlns='...'/>"), None);
    }

    #[test]
    fn a_generic_heif_brand_is_decided_by_its_compatible_brands() {
        // Every AVIF is structurally a HEIF and routinely leads with `mif1`;
        // the compatible-brand list is what tells the two apart.
        assert_eq!(
            sniff(&ftyp(*b"mif1", &[b"avif", b"miaf"])),
            Some(ImageFormat::Avif)
        );
        assert_eq!(
            sniff(&ftyp(*b"mif1", &[b"heic", b"miaf"])),
            Some(ImageFormat::Heic)
        );
        // A bare generic brand with no codec brand at all: HEIC is the bucket.
        assert_eq!(sniff(&ftyp(*b"mif1", &[])), Some(ImageFormat::Heic));
    }

    #[test]
    fn sniff_rejects_empty_and_short_buffers() {
        assert_eq!(sniff(&[]), None);
        assert_eq!(sniff(&[0x89, b'P', b'N']), None);
        assert_eq!(sniff(&[0xFF, 0xD8]), None);
        // `RIFF` alone is not enough: the `WEBP` form-type is at 8..12.
        assert_eq!(sniff(b"RIFF\x04\0\0\0"), None);
    }

    #[test]
    fn sniff_ignores_the_riff_length_field() {
        // Identification is signature-only, so a nonsense declared length is
        // still recognisably WebP. `is_complete` is what rejects it, with a
        // error that names truncation rather than an unknown container.
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
        // APP0 with a declared length but no terminator.
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

        // Cut immediately after the `IEND` *type*, before its mandatory CRC.
        // The literal is present; the chunk is not.
        let no_crc = &complete[..complete.len() - 4];
        assert!(
            !is_complete(ImageFormat::Png, no_crc),
            "the IEND literal without its CRC is not a complete chunk"
        );
        // One CRC byte short is still short.
        assert!(!is_complete(
            ImageFormat::Png,
            &complete[..complete.len() - 1]
        ));

        // `IEND` occurring inside another chunk's payload must not be mistaken
        // for the terminator — this is the false-positive direction.
        let mut decoy = super::PNG_MAGIC.to_vec();
        decoy.extend_from_slice(&8u32.to_be_bytes());
        decoy.extend_from_slice(b"IDAT");
        decoy.extend_from_slice(b"xxIENDyy");
        decoy.extend_from_slice(&[0u8; 4]);
        assert!(
            !is_complete(ImageFormat::Png, &decoy),
            "IEND inside a payload is data, not a terminator"
        );

        // A truthful chain that happens to contain the decoy still terminates.
        let mut both = decoy.clone();
        both.extend_from_slice(&0u32.to_be_bytes());
        both.extend_from_slice(b"IEND");
        both.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
        assert!(is_complete(ImageFormat::Png, &both));
    }

    /// `SOI`, one segment, then whatever the caller appends.
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
        // An `FF D9` inside an APP1 payload is metadata — an embedded EXIF
        // thumbnail is itself a JPEG and carries its own EOI — not this
        // stream's terminator.
        let decoy = jpeg_with_segment(0xE1, &[0xFF, 0xD9], &[]);
        assert!(
            !is_complete(ImageFormat::Jpeg, &decoy),
            "EOI inside an APP payload is not the stream's terminator"
        );

        // The same file with a real terminator appended is complete.
        let terminated = jpeg_with_segment(0xE1, &[0xFF, 0xD9], &[0xFF, 0xD9]);
        assert!(is_complete(ImageFormat::Jpeg, &terminated));
    }

    #[test]
    fn jpeg_completeness_walks_scan_data_byte_stuffing_aware() {
        // Inside a scan, `FF 00` is a literal 0xFF and `FF D0..D7` are restart
        // markers; neither ends the stream.
        let scan = jpeg_with_segment(
            0xDA,
            &[0x01],
            &[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xD9],
        );
        assert!(is_complete(ImageFormat::Jpeg, &scan));

        // Truncated mid-scan: the stuffed bytes are present but no EOI is.
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

    /// A minimal GIF: header, logical screen descriptor with a 6-byte global
    /// colour table, one image descriptor with a one-sub-block payload, then
    /// whatever the caller appends.
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

        // No trailer, and a cut mid-sub-block, are both incomplete — even
        // though 0x3B bytes may occur inside the data.
        assert!(!is_complete(ImageFormat::Gif, &gif_bytes(&[])));
        let complete = gif_bytes(&[0x3B]);
        assert!(!is_complete(ImageFormat::Gif, &complete[..20]));

        // A 0x3B inside a colour table is data, not the trailer.
        let mut decoy = b"GIF89a".to_vec();
        decoy.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]);
        decoy.extend_from_slice(&[0x3B; 6]); // colour table full of trailers
        assert!(!is_complete(ImageFormat::Gif, &decoy));
    }

    #[test]
    fn bmff_completeness_walks_top_level_boxes() {
        let mut bytes = ftyp(*b"avif", &[]);
        // A well-formed second box.
        bytes.extend_from_slice(&12u32.to_be_bytes());
        bytes.extend_from_slice(b"mdat");
        bytes.extend_from_slice(&[0u8; 4]);
        assert!(is_complete(ImageFormat::Avif, &bytes));

        // Cut mid-box: the declared size runs past the buffer.
        assert!(!is_complete(ImageFormat::Avif, &bytes[..bytes.len() - 2]));

        // A size-0 box extends to the end of the file and is complete by
        // definition.
        let mut open_ended = ftyp(*b"heic", &[]);
        open_ended.extend_from_slice(&0u32.to_be_bytes());
        open_ended.extend_from_slice(b"mdat");
        open_ended.extend_from_slice(&[0u8; 9]);
        assert!(is_complete(ImageFormat::Heic, &open_ended));

        // A degenerate declared size is corruption, not framing.
        let mut absurd = ftyp(*b"heic", &[]);
        absurd.extend_from_slice(&3u32.to_be_bytes());
        absurd.extend_from_slice(b"mdat");
        assert!(!is_complete(ImageFormat::Heic, &absurd));
    }
}
