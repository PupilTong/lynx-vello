//! The `lepus::Value` dynamic value type and its wire decoding.
//!
//! `lepus::Value` is Lynx's tagged dynamic value (the LepusNG analog of a JS
//! value). It appears in the config, root-lepus and custom-section payloads.
//! Strings and byte arrays borrow from the source buffer.

use crate::{
    error::{DecodeError, Result},
    reader::Reader,
};

/// A decoded `lepus::Value`.
///
/// Numeric variants are kept distinct (rather than collapsed to `f64`) so the
/// decoder is lossless with respect to the on-wire tag.
#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    /// `nil` / null.
    Nil,
    /// `undefined`.
    Undefined,
    /// Boolean.
    Bool(bool),
    /// IEEE-754 double.
    Double(f64),
    /// 32-bit signed integer.
    Int32(i32),
    /// 64-bit signed integer.
    Int64(i64),
    /// 32-bit unsigned integer.
    UInt32(u32),
    /// 64-bit unsigned integer.
    UInt64(u64),
    /// UTF-8 string borrowed from the buffer.
    Str(&'a str),
    /// Ordered array.
    Array(Vec<Value<'a>>),
    /// Ordered string-keyed table (object).
    Table(Vec<(&'a str, Value<'a>)>),
    /// Raw byte payload borrowed from the buffer.
    ByteArray(&'a [u8]),
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueTag {
    Nil = 0,
    Double = 1,
    Bool = 2,
    String = 3,
    Table = 4,
    Array = 5,
    Closure = 6,
    CFunction = 7,
    CPointer = 8,
    Int32 = 9,
    Int64 = 10,
    UInt32 = 11,
    UInt64 = 12,
    NaN = 13,
    CDate = 14,
    RegExp = 15,
    JsObject = 16,
    Undefined = 17,
    ByteArray = 18,
    RefCounted = 19,
    PrimJsValue = 20,
    FunctionTable = 21,
}

impl TryFrom<u8> for ValueTag {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Nil),
            1 => Ok(Self::Double),
            2 => Ok(Self::Bool),
            3 => Ok(Self::String),
            4 => Ok(Self::Table),
            5 => Ok(Self::Array),
            6 => Ok(Self::Closure),
            7 => Ok(Self::CFunction),
            8 => Ok(Self::CPointer),
            9 => Ok(Self::Int32),
            10 => Ok(Self::Int64),
            11 => Ok(Self::UInt32),
            12 => Ok(Self::UInt64),
            13 => Ok(Self::NaN),
            14 => Ok(Self::CDate),
            15 => Ok(Self::RegExp),
            16 => Ok(Self::JsObject),
            17 => Ok(Self::Undefined),
            18 => Ok(Self::ByteArray),
            19 => Ok(Self::RefCounted),
            20 => Ok(Self::PrimJsValue),
            21 => Ok(Self::FunctionTable),
            other => Err(DecodeError::BadValueTag(other)),
        }
    }
}

/// Decode a single `lepus::Value` at the reader's cursor.
///
/// Reference: `core/runtime/lepus/base_binary_reader.cc:240-326` and
/// `base/include/value/base_value.h:65-91`.
pub(crate) fn decode_value<'a>(r: &mut Reader<'a>) -> Result<Value<'a>> {
    let raw_tag = r.u8()?;
    let tag = ValueTag::try_from(raw_tag)?;
    match tag {
        ValueTag::Nil => Ok(Value::Nil),
        ValueTag::Undefined => Ok(Value::Undefined),
        ValueTag::Bool => Ok(Value::Bool(r.bool()?)),
        ValueTag::Double => Ok(Value::Double(r.f64()?)),
        ValueTag::Int32 => Ok(Value::Int32(r.compact_i32()?)),
        ValueTag::Int64 => Ok(Value::Int64(r.compact_u64()?.cast_signed())),
        ValueTag::UInt32 => Ok(Value::UInt32(r.compact_u32()?)),
        ValueTag::UInt64 => Ok(Value::UInt64(r.compact_u64()?)),
        ValueTag::String => Ok(Value::Str(r.lstr()?)),
        ValueTag::Array => decode_array(r),
        ValueTag::Table => decode_table(r),
        ValueTag::ByteArray => {
            let len = r.compact_u64()? as usize;
            Ok(Value::ByteArray(r.take(len)?))
        }
        ValueTag::Closure
        | ValueTag::CFunction
        | ValueTag::CPointer
        | ValueTag::NaN
        | ValueTag::CDate
        | ValueTag::RegExp
        | ValueTag::JsObject
        | ValueTag::RefCounted
        | ValueTag::PrimJsValue
        | ValueTag::FunctionTable => Err(DecodeError::BadValueTag(raw_tag)),
    }
}

fn decode_array<'a>(r: &mut Reader<'a>) -> Result<Value<'a>> {
    // C++: DecodeArray reads CompactU32 size, then values
    // (core/runtime/lepus/base_binary_reader.cc:230-237).
    let size = r.compact_u32()? as usize;
    let mut values = Vec::new();
    values
        .try_reserve(size)
        .map_err(|_| DecodeError::Malformed("array is too large"))?;
    for _ in 0..size {
        values.push(decode_value(r)?);
    }
    Ok(Value::Array(values))
}

fn decode_table<'a>(r: &mut Reader<'a>) -> Result<Value<'a>> {
    // C++: DecodeTable reads CompactU32 size, then inline key/value pairs in
    // header mode; this build has no active string table
    // (core/runtime/lepus/base_binary_reader.cc:209-229).
    let size = r.compact_u32()? as usize;
    let mut entries = Vec::new();
    entries
        .try_reserve(size)
        .map_err(|_| DecodeError::Malformed("table is too large"))?;
    for _ in 0..size {
        let key = r.lstr()?;
        let value = decode_value(r)?;
        entries.push((key, value));
    }
    Ok(Value::Table(entries))
}

#[cfg(test)]
mod tests {
    use super::{Value, decode_value};
    use crate::{error::DecodeError, reader::Reader};

    fn str_bytes(value: &str) -> Vec<u8> {
        let mut out = (value.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(value.as_bytes());
        out
    }

    fn decode(input: &[u8]) -> crate::Result<Value<'_>> {
        let mut reader = Reader::new(input);
        decode_value(&mut reader)
    }

    #[test]
    fn decodes_scalar_variants() {
        assert_eq!(decode(&[0]).unwrap(), Value::Nil);
        assert_eq!(decode(&[17]).unwrap(), Value::Undefined);
        assert_eq!(decode(&[2, 1]).unwrap(), Value::Bool(true));

        let mut double = vec![1];
        double.extend_from_slice(&1.5f64.to_le_bytes());
        assert_eq!(decode(&double).unwrap(), Value::Double(1.5));

        let mut int32 = vec![9];
        int32.extend_from_slice(&(-7i32).to_le_bytes());
        assert_eq!(decode(&int32).unwrap(), Value::Int32(-7));

        let mut int64 = vec![10];
        int64.extend_from_slice(&(-9i64).to_le_bytes());
        assert_eq!(decode(&int64).unwrap(), Value::Int64(-9));

        let mut uint32 = vec![11];
        uint32.extend_from_slice(&7u32.to_le_bytes());
        assert_eq!(decode(&uint32).unwrap(), Value::UInt32(7));

        let mut uint64 = vec![12];
        uint64.extend_from_slice(&9u64.to_le_bytes());
        assert_eq!(decode(&uint64).unwrap(), Value::UInt64(9));
    }

    #[test]
    fn decodes_string_array_table_and_byte_array() {
        let mut string = vec![3];
        string.extend_from_slice(&str_bytes("lynx"));
        assert_eq!(decode(&string).unwrap(), Value::Str("lynx"));

        let mut array = vec![5];
        array.extend_from_slice(&2u32.to_le_bytes());
        array.push(0);
        array.extend_from_slice(&[2, 0]);
        assert_eq!(
            decode(&array).unwrap(),
            Value::Array(vec![Value::Nil, Value::Bool(false)])
        );

        let mut table = vec![4];
        table.extend_from_slice(&1u32.to_le_bytes());
        table.extend_from_slice(&str_bytes("answer"));
        table.push(11);
        table.extend_from_slice(&42u32.to_le_bytes());
        assert_eq!(
            decode(&table).unwrap(),
            Value::Table(vec![("answer", Value::UInt32(42))])
        );

        let mut bytes = vec![18];
        bytes.extend_from_slice(&3u64.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        assert_eq!(decode(&bytes).unwrap(), Value::ByteArray(&[1, 2, 3]));
    }

    #[test]
    fn bad_or_truncated_value_returns_error() {
        assert_eq!(decode(&[99]).unwrap_err(), DecodeError::BadValueTag(99));
        assert!(matches!(
            decode(&[1, 0]),
            Err(DecodeError::UnexpectedEof { .. })
        ));
    }
}
