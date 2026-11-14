//! The 24-byte header Lynx puts in front of background-thread bytecode
//! (`struct Bytecode::HeaderV1` in Lynx's
//! `core/runtime/js/bytecode/quickjs/bytecode/quickjs_bytecode.h`).

use super::PrimjsError;

/// `Bytecode::BYTECODE_MAGIC`, the first little-endian `u32`.
pub const PACKED_MAGIC: u32 = 0xD8C5_4E17;
/// The only header version Lynx writes.
pub const PACKED_HEADER_VERSION: u32 = 1;
/// Size of a version-1 header: six `u32`.
pub const PACKED_HEADER_LEN: usize = 24;

/// A decoded packed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedHeader {
    /// Offset of the bytecode from the start of the buffer.
    pub bytecode_offset: u32,
    /// Length of the bytecode.
    pub bytecode_size: u32,
    /// Major component of the target SDK the bytecode was compiled for.
    pub target_sdk_major: u32,
    /// Minor component of that target SDK.
    pub target_sdk_minor: u32,
}

/// Whether `bytes` opens with the packed-header magic. `PrimJS` bytecode opens
/// with version byte `0x01` or `0x09`, so the two cannot be confused.
#[must_use]
pub fn is_packed(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(&PACKED_MAGIC.to_le_bytes()[..])
}

/// Splits a packed buffer into its header and the bytecode it wraps.
///
/// Returns `Ok(None)` when `bytes` does not open with the magic. Otherwise the
/// header version must be 1, the bytecode must start at or after the header,
/// and offset plus size must equal the buffer length, as Lynx's
/// `PrepareJavaScriptBytecode` requires. The target SDK is reported, not
/// checked.
pub fn unwrap_packed(bytes: &[u8]) -> Result<Option<(PackedHeader, &[u8])>, PrimjsError> {
    if !is_packed(bytes) {
        return Ok(None);
    }
    let bad = |reason: String| PrimjsError::BadPackedHeader { reason };
    if bytes.len() < PACKED_HEADER_LEN {
        return Err(bad(format!(
            "{} bytes is shorter than the {PACKED_HEADER_LEN}-byte header",
            bytes.len()
        )));
    }
    let word = |index: usize| {
        let at = index * 4;
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    };
    let version = word(1);
    if version != PACKED_HEADER_VERSION {
        return Err(bad(format!("header version {version}, expected 1")));
    }
    let header = PackedHeader {
        bytecode_offset: word(2),
        bytecode_size: word(3),
        target_sdk_major: word(4),
        target_sdk_minor: word(5),
    };
    let start = header.bytecode_offset as usize;
    if start < PACKED_HEADER_LEN {
        return Err(bad(format!(
            "bytecode offset {start} overlaps the {PACKED_HEADER_LEN}-byte header"
        )));
    }
    let end = start.checked_add(header.bytecode_size as usize);
    if end != Some(bytes.len()) {
        return Err(bad(format!(
            "bytecode offset {start} plus size {} does not equal the buffer length {}",
            header.bytecode_size,
            bytes.len()
        )));
    }
    Ok(Some((header, &bytes[start..])))
}
