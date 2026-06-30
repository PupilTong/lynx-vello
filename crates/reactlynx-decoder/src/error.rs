//! Decode errors.
//!
//! The decoder never panics on malformed input: every fallible read returns a
//! [`DecodeError`]. Offsets in the variants are byte positions into the buffer
//! that was handed to the decoder.

/// An error encountered while decoding a template bundle.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// A read ran past the end of the available bytes.
    #[error("unexpected end of input at byte {at}: needed {need} more byte(s), {have} available")]
    UnexpectedEof {
        /// Cursor position where the read was attempted.
        at: usize,
        /// Number of bytes the read required.
        need: usize,
        /// Number of bytes actually remaining.
        have: usize,
    },

    /// The leading magic word was not a recognized template magic.
    #[error("bad magic word 0x{0:08x}")]
    BadMagic(u32),

    /// The bundle targets the legacy Lepus VM (magic `0xdd737199`); only the
    /// LepusNG/QuickJS bundle (magic `0x00241922`) is supported.
    #[error("unsupported VM: this decoder only handles LepusNG (QuickJS) bundles")]
    UnsupportedVm,

    /// The header's declared total size did not match the buffer length.
    #[error("size mismatch: header declares {declared} bytes, buffer has {actual}")]
    SizeMismatch {
        /// `total_size` field read from the header.
        declared: u32,
        /// Actual length of the buffer.
        actual: usize,
    },

    /// A section the latest feature subset deliberately does not implement was
    /// encountered (e.g. the legacy `ELEMENT_TEMPLATE`).
    #[error("legacy/unsupported section type {0}")]
    LegacySection(u8),

    /// A `lepus::Value` carried an unknown type tag.
    #[error("unknown lepus value tag {0}")]
    BadValueTag(u8),

    /// An element node carried an unknown section tag that could not be skipped.
    #[error("unknown element section tag {0}")]
    BadElementTag(u8),

    /// A section route referenced an unknown [`crate::model`] section type.
    #[error("unknown section type {0}")]
    BadSectionType(u8),

    /// A length-prefixed string was not valid UTF-8.
    #[error("invalid utf-8 string at byte {0}")]
    Utf8(usize),

    /// The `header_ext_info` block did not start with its magic (`0x494e464f`).
    #[error("bad header_ext_info magic 0x{0:08x}")]
    BadHeaderExtMagic(u32),

    /// A structural invariant of the format was violated. The message names the
    /// invariant; it is a `&'static str` so the type stays cheap to clone.
    #[error("malformed template: {0}")]
    Malformed(&'static str),
}

/// Convenience alias for decode results.
pub type Result<T> = core::result::Result<T, DecodeError>;
