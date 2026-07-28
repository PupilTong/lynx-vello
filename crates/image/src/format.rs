//! Container identification, done in-crate from the leading bytes.
//!
//! Sniffing is deliberately ours rather than a decoder crate's: the backends
//! are chosen *per format*, so the format has to be known before any of them is
//! constructed, and the two questions a caller actually has — "which container
//! is this?" and "did all of it arrive?" — are answered by different amounts of
//! the buffer.

use std::fmt;

/// The containers this crate decodes. Static images only; see the crate docs'
/// recorded limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
}

impl ImageFormat {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
        }
    }

    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
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
/// [`Truncated`](crate::ImageError::Truncated) error instead of a useless
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
    None
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
/// there as [`Decode`](crate::ImageError::Decode).
#[must_use]
pub fn is_complete(format: ImageFormat, bytes: &[u8]) -> bool {
    match format {
        ImageFormat::Png => has_png_iend(bytes),
        // EOI. Trailing garbage after it is tolerated: cameras and stripping
        // tools routinely leave padding, and every decoder stops at the marker.
        ImageFormat::Jpeg => find_subslice(bytes, &[0xFF, 0xD9]).is_some(),
        ImageFormat::WebP => webp_payload_present(bytes),
    }
}

/// A PNG is complete once the terminating zero-length `IEND` chunk is present.
/// Scanning for the literal is enough: `IEND` carries no payload, so the
/// sequence cannot occur inside compressed data as a *chunk type* — and a false
/// positive costs only a decoder error rather than a silent bad image.
fn has_png_iend(bytes: &[u8]) -> bool {
    bytes.len() > PNG_MAGIC.len() && find_subslice(bytes, b"IEND").is_some()
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

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{ImageFormat, is_complete, sniff};

    fn png_bytes() -> Vec<u8> {
        let mut bytes = super::PNG_MAGIC.to_vec();
        bytes.extend_from_slice(b"\0\0\0\0IHDR");
        bytes.extend_from_slice(b"\0\0\0\0IEND\xAE\x42\x60\x82");
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

    #[test]
    fn sniff_identifies_each_supported_container() {
        assert_eq!(sniff(&png_bytes()), Some(ImageFormat::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageFormat::Jpeg));
        assert_eq!(sniff(&webp_bytes(b"VP8 ")), Some(ImageFormat::WebP));
    }

    #[test]
    fn sniff_rejects_containers_this_crate_does_not_decode() {
        assert_eq!(sniff(b"GIF89a......"), None);
        assert_eq!(sniff(b"BM..........").map(ImageFormat::as_str), None);
        // AVIF: an ISOBMFF `ftyp` box, not a RIFF container.
        assert_eq!(sniff(b"\0\0\0\x20ftypavif"), None);
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
        assert!(!is_complete(ImageFormat::Jpeg, &[0xFF, 0xD8, 0xFF, 0xE0]));

        assert!(is_complete(ImageFormat::WebP, &webp_bytes(b"VP8 data")));
        let short = webp_bytes(b"VP8 data");
        assert!(!is_complete(ImageFormat::WebP, &short[..short.len() - 3]));
    }

    #[test]
    fn is_complete_tolerates_trailing_bytes_after_jpeg_eoi() {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xD9];
        bytes.extend_from_slice(&[0x00; 16]);
        assert!(is_complete(ImageFormat::Jpeg, &bytes));
    }
}
