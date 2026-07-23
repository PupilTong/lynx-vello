//! Payload encodings shared by several sections.

use std::collections::BTreeMap;

use crate::DecodeError;
use crate::reader::Reader;

pub(crate) fn decode_string_map(
    bytes: &[u8],
    section: &'static str,
) -> Result<BTreeMap<String, String>, DecodeError> {
    let mut reader = Reader::new(bytes);
    let count = reader.read_u32()?;
    let mut map = BTreeMap::new();
    for _ in 0..count {
        let key = decode_utf8_field(&mut reader, section)?;
        let value = decode_utf8_field(&mut reader, section)?;
        map.insert(key, value);
    }
    if !reader.is_empty() {
        return Err(DecodeError::InvalidSection {
            section,
            reason: format!("{} trailing bytes after string map", reader.remaining()),
        });
    }
    Ok(map)
}

fn decode_utf8_field(
    reader: &mut Reader<'_>,
    section: &'static str,
) -> Result<String, DecodeError> {
    let len = reader.read_u32()? as usize;
    let bytes = reader.read_bytes(len)?;
    String::from_utf8(bytes.to_vec()).map_err(|e| DecodeError::InvalidSection {
        section,
        reason: format!("invalid UTF-8: {e}"),
    })
}

pub(crate) fn decode_utf16_json(
    bytes: &[u8],
    section: &'static str,
) -> Result<serde_json::Value, DecodeError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(DecodeError::InvalidSection {
            section,
            reason: format!("UTF-16 payload has odd length {}", bytes.len()),
        });
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let text = String::from_utf16(&units).map_err(|e| DecodeError::InvalidSection {
        section,
        reason: format!("invalid UTF-16: {e}"),
    })?;
    serde_json::from_str(&text).map_err(|source| DecodeError::Json { section, source })
}
