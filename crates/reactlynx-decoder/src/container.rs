//! Bundle envelope: header → compile options → section route → section bodies.
//!
//! This is the orchestration spine. It reads the header, validates the magic
//! and total size, parses the section route table, and dispatches each section
//! to its decoder in the canonical fiber order. Built out by the container
//! tasks; see `docs/lynx/01-container-format.md` and the implementation plan.

use crate::{
    error::{DecodeError, Result},
    model::TemplateBundle,
};

/// Decode a complete bundle from `buf`.
pub(crate) fn decode_bundle(buf: &[u8]) -> Result<TemplateBundle<'_>> {
    let _ = buf;
    Err(DecodeError::Malformed(
        "container::decode_bundle not yet implemented",
    ))
}
