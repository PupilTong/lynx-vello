use crate::ConvertError;

#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    base: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            base: 0,
        }
    }

    pub(crate) fn section(bytes: &'a [u8], start: usize, end: usize) -> Result<Self, ConvertError> {
        let section = bytes
            .get(start..end)
            .ok_or_else(|| ConvertError::invalid(start, "section range is outside the bundle"))?;
        Ok(Self {
            bytes: section,
            offset: 0,
            base: start,
        })
    }

    pub(crate) const fn position(&self) -> usize {
        self.base + self.offset
    }

    pub(crate) const fn local_position(&self) -> usize {
        self.offset
    }

    pub(crate) const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub(crate) fn set_local_position(&mut self, offset: usize) -> Result<(), ConvertError> {
        if offset > self.bytes.len() {
            return Err(ConvertError::invalid(
                self.base + offset,
                "seek is outside the containing section",
            ));
        }
        self.offset = offset;
        Ok(())
    }

    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8], ConvertError> {
        let remaining = self.remaining();
        if length > remaining {
            return Err(ConvertError::UnexpectedEof {
                offset: self.position(),
                needed: length,
                remaining,
            });
        }
        let start = self.offset;
        self.offset += length;
        Ok(&self.bytes[start..self.offset])
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ConvertError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, ConvertError> {
        let bytes: [u8; 2] = self.take(2)?.try_into().expect("checked length");
        Ok(u16::from_le_bytes(bytes))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, ConvertError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("checked length");
        Ok(u32::from_le_bytes(bytes))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, ConvertError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("checked length");
        Ok(i32::from_le_bytes(bytes))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, ConvertError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("checked length");
        Ok(u64::from_le_bytes(bytes))
    }

    pub(crate) fn f64(&mut self) -> Result<f64, ConvertError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("checked length");
        Ok(f64::from_le_bytes(bytes))
    }

    pub(crate) fn string(&mut self, context: &'static str) -> Result<String, ConvertError> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| ConvertError::invalid(self.position(), "string length overflow"))?;
        let offset = self.position();
        let bytes = self.take(length)?;
        let value = std::str::from_utf8(bytes).map_err(|source| ConvertError::InvalidUtf8 {
            context,
            offset,
            source,
        })?;
        Ok(value.to_owned())
    }
}
