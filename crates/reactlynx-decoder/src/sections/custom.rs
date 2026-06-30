//! `CUSTOM_SECTIONS` section.

use crate::{
    error::{DecodeError, Result},
    model::{CustomSection, TemplateBundle},
    reader::Reader,
    value::{Value, decode_value},
};

pub(crate) fn decode<'a>(reader: &mut Reader<'a>, bundle: &mut TemplateBundle<'a>) -> Result<()> {
    // C++: DecodeCustomSectionsSection reads U32 route size, then key, header
    // value, start, end entries (lynx_binary_reader.cc:281-302).
    let count = reader.u32()? as usize;
    let mut routes = Vec::new();
    routes
        .try_reserve(count)
        .map_err(|_| DecodeError::Malformed("custom section route is too large"))?;
    for _ in 0..count {
        let key = reader.lstr()?;
        let header = decode_value(reader)?;
        let start = reader.u32()? as usize;
        let end = reader.u32()? as usize;
        routes.push((key, header, start, end));
    }

    let descriptor_offset = reader.pos();
    bundle
        .custom_sections
        .try_reserve(routes.len())
        .map_err(|_| DecodeError::Malformed("custom sections are too large"))?;
    for (key, header, start, end) in routes {
        if start > end {
            return Err(DecodeError::Malformed("custom section route is inverted"));
        }
        let absolute = descriptor_offset
            .checked_add(start)
            .ok_or(DecodeError::Malformed("custom section start overflow"))?;
        let mut content_reader = reader.clone();
        content_reader.seek(absolute)?;
        let content = match custom_encoding(&header) {
            CustomEncoding::String | CustomEncoding::Css => decode_value(&mut content_reader)?,
            CustomEncoding::JsBytecode => {
                let len = content_reader.compact_u64()? as usize;
                Value::ByteArray(content_reader.take(len)?)
            }
        };
        bundle.custom_sections.push(CustomSection {
            key,
            header,
            content,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomEncoding {
    String,
    JsBytecode,
    Css,
}

fn custom_encoding(header: &Value<'_>) -> CustomEncoding {
    if let Value::Table(entries) = header {
        for (key, value) in entries {
            if *key == "encoding" {
                return match numeric_value(value) {
                    Some(1) => CustomEncoding::JsBytecode,
                    Some(2) => CustomEncoding::Css,
                    _ => CustomEncoding::String,
                };
            }
        }
    }
    CustomEncoding::String
}

fn numeric_value(value: &Value<'_>) -> Option<u64> {
    match value {
        Value::Int32(v) => u64::try_from(*v).ok(),
        Value::Int64(v) => u64::try_from(*v).ok(),
        Value::UInt32(v) => Some(u64::from(*v)),
        Value::UInt64(v) => Some(*v),
        _ => None,
    }
}
