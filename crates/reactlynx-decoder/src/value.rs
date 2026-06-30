//! The `lepus::Value` dynamic value type and its wire decoding.
//!
//! `lepus::Value` is Lynx's tagged dynamic value (the LepusNG analog of a JS
//! value). It appears in the config, root-lepus and custom-section payloads.
//! Strings and byte arrays borrow from the source buffer.

use crate::{error::Result, reader::Reader};

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

/// Decode a single `lepus::Value` at the reader's cursor.
///
/// Reference: `core/runtime/vm/lepus/binary_reader` value decoding and
/// `base/include/value/base_value.h`. Filled in by the value-decode task.
pub(crate) fn decode_value<'a>(r: &mut Reader<'a>) -> Result<Value<'a>> {
    let _ = r;
    Err(crate::error::DecodeError::Malformed(
        "value::decode_value not yet implemented",
    ))
}
