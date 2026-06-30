//! A bounds-checked, zero-copy cursor over the template byte buffer.
//!
//! Every method advances [`Reader::pos`] and returns [`DecodeError::UnexpectedEof`]
//! rather than panicking when the buffer is too short. Borrowed reads
//! ([`Reader::lstr`], [`Reader::take`]) return slices tied to the original
//! buffer lifetime so the decoder stays allocation-free for raw payloads.
//!
//! ## Fixed-width "compact" integers
//!
//! The Lynx codec's `WriteCompactU32` / `ReadCompactU32` family is, despite a
//! vestigial `// leb128` comment in the C++, a **fixed-width little-endian**
//! encoding: `compact_u32`/`compact_i32` are 4 bytes, `compact_u64`/`compact_f64`
//! are 8 bytes. The `compact_*` methods are kept distinct from the plain
//! fixed-width readers so that call sites mirror the C++ `WriteCompact*` vs
//! `WriteU32` distinction; if a future device build switches the compact family
//! to ULEB128, this module is the single place to change.

use crate::error::{DecodeError, Result};

/// A forward cursor over `&'a [u8]` with bounds-checked reads.
#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Wrap a buffer, positioned at byte 0.
    pub(crate) const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Current cursor position.
    pub(crate) const fn pos(&self) -> usize {
        self.pos
    }

    /// Total length of the underlying buffer.
    pub(crate) const fn len(&self) -> usize {
        self.buf.len()
    }

    /// Bytes left between the cursor and the end of the buffer.
    pub(crate) const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Whether the cursor has reached the end of the buffer.
    pub(crate) const fn is_at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// The whole underlying buffer (ignores the cursor).
    pub(crate) const fn buffer(&self) -> &'a [u8] {
        self.buf
    }

    fn need(&self, n: usize) -> Result<()> {
        if self.remaining() < n {
            Err(DecodeError::UnexpectedEof {
                at: self.pos,
                need: n,
                have: self.remaining(),
            })
        } else {
            Ok(())
        }
    }

    /// Borrow the next `n` bytes and advance.
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        self.need(n)?;
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Advance by `n` bytes without returning them.
    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        self.need(n)?;
        self.pos += n;
        Ok(())
    }

    /// Move the cursor to an absolute position (must be within the buffer).
    pub(crate) fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.buf.len() {
            return Err(DecodeError::UnexpectedEof {
                at: pos,
                need: 0,
                have: self.buf.len().saturating_sub(pos),
            });
        }
        self.pos = pos;
        Ok(())
    }

    /// A fresh sub-reader over the absolute byte range `[start, end)` of the
    /// underlying buffer. Used to decode a section from its route range
    /// independently of the outer cursor.
    pub(crate) fn sub(&self, start: usize, end: usize) -> Result<Reader<'a>> {
        if start > end || end > self.buf.len() {
            return Err(DecodeError::Malformed("section range out of bounds"));
        }
        Ok(Reader::new(&self.buf[start..end]))
    }

    /// One byte.
    pub(crate) fn u8(&mut self) -> Result<u8> {
        self.need(1)?;
        let b = self.buf[self.pos];
        self.pos += 1;
        Ok(b)
    }

    /// One byte interpreted as a boolean (`0` is false, anything else true).
    pub(crate) fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    /// Fixed 4-byte little-endian `u32` (the codec's `WriteU32`).
    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Fixed 2-byte little-endian `u16`.
    pub(crate) fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// Fixed 4-byte little-endian `i32`.
    pub(crate) fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()?.cast_signed())
    }

    /// Fixed 8-byte little-endian `u64`.
    pub(crate) fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Fixed 8-byte little-endian `i64`.
    pub(crate) fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()?.cast_signed())
    }

    /// Fixed 8-byte IEEE-754 little-endian `f64`.
    pub(crate) fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// `WriteCompactU32` — fixed 4-byte little-endian in this format.
    pub(crate) fn compact_u32(&mut self) -> Result<u32> {
        self.u32()
    }

    /// `WriteCompactS32` — fixed 4-byte little-endian, two's complement.
    pub(crate) fn compact_i32(&mut self) -> Result<i32> {
        self.i32()
    }

    /// `WriteCompactU64` — fixed 8-byte little-endian.
    pub(crate) fn compact_u64(&mut self) -> Result<u64> {
        self.u64()
    }

    /// `WriteCompactD64` — fixed 8-byte IEEE-754 little-endian.
    pub(crate) fn compact_f64(&mut self) -> Result<f64> {
        self.f64()
    }

    /// A length-prefixed UTF-8 string: `compact_u32 len` then `len` bytes,
    /// borrowed from the buffer.
    pub(crate) fn lstr(&mut self) -> Result<&'a str> {
        let len = self.compact_u32()? as usize;
        let at = self.pos;
        let bytes = self.take(len)?;
        core::str::from_utf8(bytes).map_err(|_| DecodeError::Utf8(at))
    }

    /// A length-prefixed raw byte payload: `compact_u32 len` then `len` bytes.
    pub(crate) fn lbytes(&mut self) -> Result<&'a [u8]> {
        let len = self.compact_u32()? as usize;
        self.take(len)
    }
}

#[cfg(test)]
mod tests {
    use super::Reader;
    use crate::error::DecodeError;

    #[test]
    fn compact_u32_is_4_byte_le() {
        // Vector from the C++ binary_input_stream unit test: the ASCII bytes
        // "test" read as a CompactU32 yield 0x74736574.
        let mut r = Reader::new(b"test");
        assert_eq!(r.compact_u32().unwrap(), 0x7473_6574);
        assert!(r.is_at_end());
    }

    #[test]
    fn compact_u64_is_8_byte_le() {
        // "test str" read as a CompactU64.
        let mut r = Reader::new(b"test str");
        assert_eq!(r.compact_u64().unwrap(), 0x7274_7320_7473_6574);
        assert!(r.is_at_end());
    }

    #[test]
    fn u16_is_2_byte_le() {
        let mut r = Reader::new(&[0x34, 0x12]);
        assert_eq!(r.u16().unwrap(), 0x1234);
        assert!(r.is_at_end());
    }

    #[test]
    fn f64_roundtrips_bit_exact() {
        let bytes = core::f64::consts::PI.to_le_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.f64().unwrap().to_bits(), core::f64::consts::PI.to_bits());
    }

    #[test]
    fn lstr_reads_length_prefixed_utf8() {
        // [len=5 LE][b"hello"]
        let mut buf = 5u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"hello");
        let mut r = Reader::new(&buf);
        assert_eq!(r.lstr().unwrap(), "hello");
        assert!(r.is_at_end());
    }

    #[test]
    fn short_read_yields_eof_not_panic() {
        let mut r = Reader::new(&[0x01, 0x02]);
        let err = r.u32().unwrap_err();
        assert_eq!(
            err,
            DecodeError::UnexpectedEof {
                at: 0,
                need: 4,
                have: 2
            }
        );
        // Cursor is unmoved after a failed read.
        assert_eq!(r.pos(), 0);
    }

    #[test]
    fn bad_utf8_reports_offset() {
        let mut buf = 1u32.to_le_bytes().to_vec();
        buf.push(0xFF);
        let mut r = Reader::new(&buf);
        assert_eq!(r.lstr().unwrap_err(), DecodeError::Utf8(4));
    }

    #[test]
    fn sub_reader_is_independent() {
        let buf: Vec<u8> = (0..16).collect();
        let r = Reader::new(&buf);
        let mut s = r.sub(4, 8).unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s.u8().unwrap(), 4);
        // out-of-range sub is rejected, not panicked
        assert!(r.sub(8, 4).is_err());
        assert!(r.sub(0, 99).is_err());
    }
}
