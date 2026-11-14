//! Byte cursor with the primitive encodings of `PrimJS`'s `bc_get_*` readers.

use super::PrimjsError;

#[derive(Debug, Clone)]
pub(super) struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(super) const fn offset(&self) -> usize {
        self.offset
    }

    pub(super) const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    pub(super) fn take(&mut self, length: usize) -> Result<&'a [u8], PrimjsError> {
        let remaining = self.remaining();
        if length > remaining {
            return Err(PrimjsError::UnexpectedEof {
                offset: self.offset,
                needed: length,
                remaining,
            });
        }
        let start = self.offset;
        self.offset += length;
        Ok(&self.bytes[start..self.offset])
    }

    pub(super) fn u8(&mut self) -> Result<u8, PrimjsError> {
        Ok(self.take(1)?[0])
    }

    pub(super) fn u16(&mut self) -> Result<u16, PrimjsError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(super) fn u64(&mut self) -> Result<u64, PrimjsError> {
        let bytes = self.take(8)?;
        let mut word = [0; 8];
        word.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(word))
    }

    /// `get_leb128`: at most five bytes. `PrimJS` drops the bits of a fifth
    /// byte above 32; this reader rejects them instead.
    pub(super) fn leb128(&mut self) -> Result<u32, PrimjsError> {
        let start = self.offset;
        let mut value: u32 = 0;
        for index in 0..5 {
            let byte = self.u8()?;
            if index == 4 && byte & 0x70 != 0 {
                return Err(PrimjsError::BadLeb128 { offset: start });
            }
            value |= u32::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(PrimjsError::BadLeb128 { offset: start })
    }

    /// `get_sleb128`: a zigzag-encoded LEB128 value.
    pub(super) fn sleb128(&mut self) -> Result<i32, PrimjsError> {
        let raw = self.leb128()?;
        Ok((raw >> 1).cast_signed() ^ -(raw & 1).cast_signed())
    }
}
