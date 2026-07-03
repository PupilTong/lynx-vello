//! Decode errors.

/// Errors produced while decoding a Lynx web binary template bundle.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The input does not start with the `SDRA WROF` magic pair.
    #[error("not a Lynx web binary template: magic 0x{magic0:08x} 0x{magic1:08x}")]
    BadMagic {
        /// First magic word found (expected `0x4152_4453`, "SDRA").
        magic0: u32,
        /// Second magic word found (expected `0x464F_5257`, "WROF").
        magic1: u32,
    },
    /// The bundle declares a format version newer than this decoder supports.
    #[error("unsupported template version {0} (this decoder supports <= 1)")]
    UnsupportedVersion(u32),
    /// The input ended before a complete value could be read.
    #[error("unexpected end of input at offset {offset} (needed {needed} more bytes)")]
    UnexpectedEof {
        /// Byte offset at which more data was required.
        offset: usize,
        /// Number of bytes that were needed but missing.
        needed: usize,
    },
    /// A section carried a label this decoder does not know about.
    ///
    /// The reference decoder (`web-core` `decodeTemplate`) also treats unknown
    /// labels as an error.
    #[error("unknown section label {0}")]
    UnknownSection(u32),
    /// A section body is malformed.
    #[error("invalid {section} section: {reason}")]
    InvalidSection {
        /// Which section was being decoded.
        section: &'static str,
        /// What went wrong.
        reason: String,
    },
    /// A JSON-encoded section did not parse.
    #[error("invalid JSON in {section} section")]
    Json {
        /// Which section was being decoded.
        section: &'static str,
        /// The underlying parse error.
        #[source]
        source: serde_json::Error,
    },
    /// The rkyv-encoded `StyleInfo` section failed validation or
    /// deserialization.
    #[error("invalid StyleInfo section: {0}")]
    StyleInfo(String),
}
