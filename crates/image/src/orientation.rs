//! EXIF orientation, read from JPEG metadata and applied to decoded pixels.
//!
//! css-images-3 makes `image-orientation: from-image` the initial value, and the
//! stylo fork's lynx grammar has no `image-orientation` property at all — so
//! there is no way for an author to ask for un-oriented pixels, and applying the
//! tag is the only correct behaviour. It also happens to be the only way the
//! backends can agree: `AImageDecoder` orients unconditionally while the
//! software decoders and `CGImageSourceCreateImageAtIndex` do not, so a backend
//! that skipped this would disagree with Android about what a photo looks like.
//!
//! JPEG only. PNG's `eXIf` chunk and WebP's `EXIF` chunk are not read — both are
//! rare in practice and neither carries the camera-capture orientation this
//! exists for. That is a recorded v1 limit.

/// The eight EXIF orientation values (tag `0x0112`), as the transform each one
/// asks for. Values outside 1..=8 are treated as [`Self::Identity`], which is
/// what every mainstream decoder does with a corrupt tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Orientation {
    #[default]
    Identity,
    FlipHorizontal,
    Rotate180,
    FlipVertical,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}

impl Orientation {
    fn from_exif(value: u16) -> Self {
        match value {
            2 => Self::FlipHorizontal,
            3 => Self::Rotate180,
            4 => Self::FlipVertical,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => Self::Identity,
        }
    }

    /// Whether the transform exchanges the width and height axes.
    pub(crate) const fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270
        )
    }

    /// The oriented size for a stored `width` x `height`.
    pub(crate) const fn apply_to_size(self, width: u32, height: u32) -> (u32, u32) {
        if self.swaps_axes() {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// Rewrites an RGBA8 buffer into oriented order, returning the new
    /// dimensions. [`Self::Identity`] returns the buffer untouched, so the
    /// overwhelmingly common case costs one branch and no allocation.
    pub(crate) fn apply(self, pixels: Vec<u8>, width: u32, height: u32) -> (Vec<u8>, u32, u32) {
        if self == Self::Identity {
            return (pixels, width, height);
        }
        let (out_width, out_height) = self.apply_to_size(width, height);
        let (source_width, source_height) = (width as usize, height as usize);
        let mut out = vec![0u8; pixels.len()];

        for y in 0..source_height {
            for x in 0..source_width {
                // Destination coordinates for the source pixel at (x, y).
                let (dx, dy) = match self {
                    Self::Identity => (x, y),
                    Self::FlipHorizontal => (source_width - 1 - x, y),
                    Self::Rotate180 => (source_width - 1 - x, source_height - 1 - y),
                    Self::FlipVertical => (x, source_height - 1 - y),
                    Self::Transpose => (y, x),
                    Self::Rotate90 => (source_height - 1 - y, x),
                    Self::Transverse => (source_height - 1 - y, source_width - 1 - x),
                    Self::Rotate270 => (y, source_width - 1 - x),
                };
                let from = (y * source_width + x) * 4;
                let to = (dy * out_width as usize + dx) * 4;
                out[to..to + 4].copy_from_slice(&pixels[from..from + 4]);
            }
        }
        (out, out_width, out_height)
    }
}

/// Reads the orientation tag out of a JPEG's EXIF APP1 segment.
///
/// Walks the marker chain rather than scanning for the `Exif\0\0` literal:
/// entropy-coded scan data can contain any byte sequence, so a naive search can
/// match inside compressed pixels. Stops at the first scan (`SOS`), by which
/// point every metadata segment has been seen.
pub(crate) fn jpeg_orientation(bytes: &[u8]) -> Orientation {
    let Some(app1) = find_exif_app1(bytes) else {
        return Orientation::Identity;
    };
    parse_exif_orientation(app1).unwrap_or_default()
}

fn find_exif_app1(bytes: &[u8]) -> Option<&[u8]> {
    // Past SOI.
    let mut cursor = 2usize;
    loop {
        // Markers may be preceded by any number of 0xFF fill bytes.
        while bytes.get(cursor) == Some(&0xFF) && bytes.get(cursor + 1) == Some(&0xFF) {
            cursor += 1;
        }
        if bytes.get(cursor)? != &0xFF {
            return None;
        }
        let marker = *bytes.get(cursor + 1)?;
        // SOS or EOI: metadata is behind us.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let high = *bytes.get(cursor + 2)?;
        let low = *bytes.get(cursor + 3)?;
        let length = usize::from(u16::from_be_bytes([high, low]));
        if length < 2 {
            return None;
        }
        let payload_start = cursor + 4;
        let payload_end = cursor + 2 + length;
        let payload = bytes.get(payload_start..payload_end)?;
        if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
            return payload.get(6..);
        }
        cursor = payload_end;
    }
}

/// Parses a TIFF header plus IFD0 far enough to read tag `0x0112`.
fn parse_exif_orientation(tiff: &[u8]) -> Option<Orientation> {
    let little_endian = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let read_u16 = |at: usize| -> Option<u16> {
        let pair = tiff.get(at..at + 2)?;
        let pair = [pair[0], pair[1]];
        Some(if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    };
    let read_u32 = |at: usize| -> Option<u32> {
        let quad = tiff.get(at..at + 4)?;
        let quad = [quad[0], quad[1], quad[2], quad[3]];
        Some(if little_endian {
            u32::from_le_bytes(quad)
        } else {
            u32::from_be_bytes(quad)
        })
    };

    if read_u16(2)? != 42 {
        return None;
    }
    let ifd0 = read_u32(4)? as usize;
    let count = read_u16(ifd0)?;
    for index in 0..usize::from(count) {
        let entry = ifd0 + 2 + index * 12;
        if read_u16(entry)? == 0x0112 {
            // A SHORT value sits in the first two bytes of the value field,
            // in the file's own byte order.
            return Some(Orientation::from_exif(read_u16(entry + 8)?));
        }
    }
    None
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{Orientation, jpeg_orientation};

    /// A minimal JPEG: SOI, an EXIF APP1 carrying one IFD0 entry, then SOS.
    fn jpeg_with_orientation(value: u16, little_endian: bool) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(if little_endian { b"II" } else { b"MM" });
        let u16_bytes = |v: u16| {
            if little_endian {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32_bytes = |v: u32| {
            if little_endian {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        tiff.extend_from_slice(&u16_bytes(42));
        tiff.extend_from_slice(&u32_bytes(8)); // IFD0 at offset 8
        tiff.extend_from_slice(&u16_bytes(1)); // one entry
        tiff.extend_from_slice(&u16_bytes(0x0112)); // tag
        tiff.extend_from_slice(&u16_bytes(3)); // SHORT
        tiff.extend_from_slice(&u32_bytes(1)); // count
        tiff.extend_from_slice(&u16_bytes(value));
        tiff.extend_from_slice(&[0, 0]); // value field padding
        tiff.extend_from_slice(&u32_bytes(0)); // no next IFD

        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);

        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        #[allow(clippy::cast_possible_truncation)]
        let length = (payload.len() + 2) as u16;
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend_from_slice(&payload);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        jpeg
    }

    #[test]
    fn reads_the_orientation_tag_in_both_byte_orders() {
        for little_endian in [true, false] {
            assert_eq!(
                jpeg_orientation(&jpeg_with_orientation(6, little_endian)),
                Orientation::Rotate90
            );
            assert_eq!(
                jpeg_orientation(&jpeg_with_orientation(1, little_endian)),
                Orientation::Identity
            );
        }
    }

    #[test]
    fn treats_missing_or_nonsense_metadata_as_identity() {
        assert_eq!(
            jpeg_orientation(&[0xFF, 0xD8, 0xFF, 0xDA]),
            Orientation::Identity
        );
        assert_eq!(jpeg_orientation(&[]), Orientation::Identity);
        assert_eq!(
            jpeg_orientation(&jpeg_with_orientation(99, true)),
            Orientation::Identity
        );
    }

    #[test]
    fn rotate90_moves_the_top_left_pixel_to_the_top_right() {
        // 2x1: [A][B] rotated 90° CW becomes 1x2 with A on top... in
        // destination terms, source (0,0) lands at (height-1-y, x) = (0, 0)
        // and source (1,0) lands at (0, 1).
        let pixels = vec![
            1, 1, 1, 255, // A
            2, 2, 2, 255, // B
        ];
        let (out, width, height) = Orientation::Rotate90.apply(pixels, 2, 1);
        assert_eq!((width, height), (1, 2));
        assert_eq!(&out[0..4], &[1, 1, 1, 255]);
        assert_eq!(&out[4..8], &[2, 2, 2, 255]);
    }

    #[test]
    fn identity_returns_the_buffer_untouched() {
        let pixels = vec![9u8; 16];
        let (out, width, height) = Orientation::Identity.apply(pixels.clone(), 2, 2);
        assert_eq!(out, pixels);
        assert_eq!((width, height), (2, 2));
    }

    #[test]
    fn flips_are_their_own_inverse() {
        let pixels: Vec<u8> = (0u8..(3 * 2 * 4)).collect();
        for flip in [
            Orientation::FlipHorizontal,
            Orientation::FlipVertical,
            Orientation::Rotate180,
        ] {
            let (once, width, height) = flip.apply(pixels.clone(), 3, 2);
            let (twice, width2, height2) = flip.apply(once, width, height);
            assert_eq!(twice, pixels, "{flip:?} applied twice is the identity");
            assert_eq!((width2, height2), (3, 2));
        }
    }

    #[test]
    fn axis_swapping_transforms_report_swapped_sizes() {
        for orientation in [
            Orientation::Rotate90,
            Orientation::Rotate270,
            Orientation::Transpose,
            Orientation::Transverse,
        ] {
            assert!(orientation.swaps_axes());
            assert_eq!(orientation.apply_to_size(16, 8), (8, 16));
        }
        assert!(!Orientation::Rotate180.swaps_axes());
        assert_eq!(Orientation::Rotate180.apply_to_size(16, 8), (16, 8));
    }
}
