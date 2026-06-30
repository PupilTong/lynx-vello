//! The decoded data model — the shapes `decode_template` produces.
//!
//! All borrowed data ties to the input buffer lifetime so decoding stays
//! allocation-light. The full model (header, compile options, element tree,
//! styles, sections) is built out by the section-decoder tasks; see
//! `docs/lynx/06-decoder-implementation-plan.md` §3.

/// A fully decoded native template bundle.
///
/// Currently a placeholder exposing the raw buffer; the section fields are
/// added as their decoders land.
#[derive(Debug, Clone)]
pub struct TemplateBundle<'a> {
    /// The raw bundle bytes the rest of the model borrows from.
    pub raw: &'a [u8],
}
