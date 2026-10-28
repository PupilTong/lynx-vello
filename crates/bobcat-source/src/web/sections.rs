//! Payload encodings shared by several sections.

use std::collections::BTreeMap;

use super::DecodeError;
use super::reader::Reader;

/// A binary string map, each key and value borrowed from the section.
pub(crate) fn decode_string_map<'a>(
    bytes: &'a [u8],
    section: &'static str,
) -> Result<BTreeMap<&'a str, &'a str>, DecodeError> {
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

/// Whether a binary string map names `key`, read from its keys alone; a map
/// too short for what it declares names what it holds before the end.
pub(crate) fn string_map_has_key(bytes: &[u8], key: &[u8]) -> bool {
    let mut reader = Reader::new(bytes);
    let Ok(count) = reader.read_u32() else {
        return false;
    };
    for _ in 0..count {
        let Ok(name) = reader
            .read_u32()
            .and_then(|len| reader.read_bytes(len as usize))
        else {
            return false;
        };
        if name == key {
            return true;
        }
        if reader
            .read_u32()
            .and_then(|len| reader.read_bytes(len as usize))
            .is_err()
        {
            return false;
        }
    }
    false
}

fn decode_utf8_field<'a>(
    reader: &mut Reader<'a>,
    section: &'static str,
) -> Result<&'a str, DecodeError> {
    let len = reader.read_u32()? as usize;
    let bytes = reader.read_bytes(len)?;
    std::str::from_utf8(bytes).map_err(|e| DecodeError::InvalidSection {
        section,
        reason: format!("invalid UTF-8: {e}"),
    })
}

/// A UTF-16LE section's text, as UTF-8.
pub(crate) fn decode_utf16_text(
    bytes: &[u8],
    section: &'static str,
) -> Result<String, DecodeError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(DecodeError::InvalidSection {
            section,
            reason: format!("UTF-16 payload has odd length {}", bytes.len()),
        });
    }
    let units = bytes.as_chunks::<2>().0;
    // The compiler's JSON is ASCII but for what its strings hold, and ASCII
    // is its own UTF-8.
    if units.iter().all(|&[low, high]| high == 0 && low.is_ascii()) {
        let ascii = units.iter().map(|&[low, _]| low).collect();
        return Ok(String::from_utf8(ascii).expect("ASCII is UTF-8"));
    }
    let units: Vec<u16> = units.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    String::from_utf16(&units).map_err(|e| DecodeError::InvalidSection {
        section,
        reason: format!("invalid UTF-16: {e}"),
    })
}

pub(crate) fn decode_utf16_json(
    bytes: &[u8],
    section: &'static str,
) -> Result<serde_json::Value, DecodeError> {
    let text = decode_utf16_text(bytes, section)?;
    serde_json::from_str(&text).map_err(|source| DecodeError::Json { section, source })
}
