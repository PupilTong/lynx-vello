//! Container-level animation declarations, read from the bytes.
//!
//! Exists because no platform API answers "is this animated?" both reliably
//! and everywhere it is needed: `AImageDecoder_isAnimated` is **API 31** while
//! the Android decoder's floor is 30, and `ImageIO`'s frame count misses an
//! APNG whose animation has a single frame (the `acTL` is present, the count
//! is 1). The two chunk-level facts below are exactly what `png` and
//! `image-webp` report, so every decoder that consults this module agrees with
//! the Linux reference about the same file.

use bobcat_core::image::ImageFormat;

/// Whether the container declares animation, regardless of frame count.
pub(crate) fn container_declares_animation(format: ImageFormat, bytes: &[u8]) -> bool {
    match format {
        ImageFormat::Png => png_has_actl(bytes),
        ImageFormat::WebP => webp_has_anim(bytes),
        // JPEG has no animation extension; GIF, HEIC and AVIF declare
        // animation through their frame/item counts, which the decoders that
        // claim them read from the platform.
        _ => false,
    }
}

/// An APNG is a PNG carrying an `acTL` chunk, which the spec requires to appear
/// before the first `IDAT`. Walking the chunk table rather than scanning for the
/// literal is what keeps compressed pixel data from producing a false positive.
fn png_has_actl(bytes: &[u8]) -> bool {
    // Past the 8-byte signature.
    let mut cursor = 8usize;
    while let Some(header) = bytes.get(cursor..cursor + 8) {
        let Ok(length) = usize::try_from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ])) else {
            return false;
        };
        match &header[4..8] {
            b"acTL" => return true,
            // Image data has started; `acTL` can no longer legally appear.
            b"IDAT" => return false,
            _ => {}
        }
        // 4 length + 4 type + payload + 4 CRC.
        let Some(next) = cursor
            .checked_add(12)
            .and_then(|end| end.checked_add(length))
        else {
            return false;
        };
        cursor = next;
    }
    false
}

/// An animated WebP is an extended (`VP8X`) file with the animation feature bit
/// set, carrying `ANIM`/`ANMF` chunks. Either signal is enough.
fn webp_has_anim(bytes: &[u8]) -> bool {
    // Past `RIFF`, the payload length, and the `WEBP` form type.
    let mut cursor = 12usize;
    while let Some(header) = bytes.get(cursor..cursor + 8) {
        let Ok(length) = usize::try_from(u32::from_le_bytes([
            header[4], header[5], header[6], header[7],
        ])) else {
            return false;
        };
        match &header[0..4] {
            b"ANIM" | b"ANMF" => return true,
            // Bit 1 of the feature-flags byte is ANIMATION.
            b"VP8X" if bytes.get(cursor + 8).is_some_and(|flags| flags & 0x02 != 0) => {
                return true;
            }
            _ => {}
        }
        // RIFF chunks are padded to an even length.
        let Some(next) = length
            .checked_add(length & 1)
            .and_then(|padded| padded.checked_add(8))
            .and_then(|advance| cursor.checked_add(advance))
        else {
            return false;
        };
        cursor = next;
    }
    false
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use bobcat_core::image::ImageFormat;

    use super::{container_declares_animation, png_has_actl, webp_has_anim};

    /// A PNG signature followed by the given chunks, each `(type, payload)`.
    fn png(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        for (kind, payload) in chunks {
            #[allow(clippy::cast_possible_truncation)]
            bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            bytes.extend_from_slice(*kind);
            bytes.extend_from_slice(payload);
            bytes.extend_from_slice(&[0, 0, 0, 0]); // CRC, never checked here
        }
        bytes
    }

    #[test]
    fn an_actl_chunk_before_the_image_data_marks_an_apng() {
        let apng = png(&[
            (b"IHDR", &[0u8; 13]),
            (b"acTL", &[0u8; 8]),
            (b"IDAT", &[0u8; 4]),
        ]);
        assert!(png_has_actl(&apng));
        assert!(container_declares_animation(ImageFormat::Png, &apng));
    }

    #[test]
    fn a_still_png_is_not_animated_even_if_its_pixels_spell_actl() {
        // The literal inside compressed data is why this walks the chunk table
        // instead of scanning: a naive search would report animation here.
        let still = png(&[(b"IHDR", &[0u8; 13]), (b"IDAT", b"...acTL...")]);
        assert!(!png_has_actl(&still));
        // A truncated chunk table runs out rather than looping or panicking.
        assert!(!png_has_actl(&still[..12]));
        assert!(!png_has_actl(&[]));
    }

    /// A RIFF/WEBP container holding the given `(fourcc, payload)` chunks.
    fn webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut payload = b"WEBP".to_vec();
        for (fourcc, data) in chunks {
            payload.extend_from_slice(*fourcc);
            #[allow(clippy::cast_possible_truncation)]
            payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
            payload.extend_from_slice(data);
            if data.len() % 2 == 1 {
                payload.push(0);
            }
        }
        let mut bytes = b"RIFF".to_vec();
        #[allow(clippy::cast_possible_truncation)]
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        bytes
    }

    #[test]
    fn the_vp8x_animation_bit_and_the_anim_chunk_both_count() {
        let flagged = webp(&[(b"VP8X", &[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0])]);
        assert!(webp_has_anim(&flagged));

        let chunked = webp(&[
            (b"VP8X", &[0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (b"ANIM", &[0u8; 6]),
        ]);
        assert!(webp_has_anim(&chunked));
        assert!(container_declares_animation(ImageFormat::WebP, &chunked));
    }

    #[test]
    fn a_still_webp_is_not_animated() {
        // An odd-length chunk exercises the RIFF even-padding rule: mis-walking
        // it would desynchronise every later chunk header.
        let still = webp(&[(b"VP8 ", &[0u8; 7]), (b"ALPH", &[0u8; 4])]);
        assert!(!webp_has_anim(&still));
        assert!(!webp_has_anim(&still[..10]));
        // JPEG has no animation extension at all.
        assert!(!container_declares_animation(ImageFormat::Jpeg, &still));
    }
}
