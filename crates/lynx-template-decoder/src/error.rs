//! Decode errors.

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("not a Lynx web binary template: magic 0x{magic0:08x} 0x{magic1:08x}")]
    BadMagic { magic0: u32, magic1: u32 },
    #[error("unsupported template version {0} (this decoder supports <= 1)")]
    UnsupportedVersion(u32),
    #[error("unexpected end of input at offset {offset} (needed {needed} more bytes)")]
    UnexpectedEof { offset: usize, needed: usize },
    #[error("unknown section label {0}")]
    UnknownSection(u32),
    #[error("invalid {section} section: {reason}")]
    InvalidSection {
        section: &'static str,
        reason: String,
    },
    #[error("invalid JSON in {section} section")]
    Json {
        section: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid StyleInfo section: {0}")]
    StyleInfo(String),
}
