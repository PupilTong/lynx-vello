//! Decoder for the ReactLynx **native binary template** format
//! (template-bundle magic `0x00241922`, LepusNG/QuickJS).
//!
//! This crate targets the *latest* feature subset only: where the encoder gates
//! behavior behind a switch, we decode the new path (`NEW_ELEMENT_TEMPLATE`,
//! `STYLE_OBJECT`, parsed styles) and intentionally do not implement legacy
//! paths (radon / virtual-node tree, the old `ELEMENT_TEMPLATE` section, TTML
//! page parsing, Air mode). See `docs/lynx/` for the format reference.
//!
//! # Example
//!
//! ```no_run
//! let bytes = std::fs::read("app.lynx").unwrap();
//! match reactlynx_decoder::decode_template(&bytes) {
//!     Ok(bundle) => println!("decoded {} bytes", bundle.raw.len()),
//!     Err(e) => eprintln!("decode failed: {e}"),
//! }
//! ```

mod container;
pub mod error;
pub mod model;
pub(crate) mod reader;
pub(crate) mod sections;
pub mod value;
pub mod version;

pub use error::{DecodeError, Result};
pub use model::TemplateBundle;
pub use value::Value;
pub use version::Version;

/// Decode a native template bundle, borrowing from `buf`.
///
/// # Errors
///
/// Returns a [`DecodeError`] for an unrecognized magic, a size mismatch, a
/// truncated or malformed body, or a legacy section that this decoder does not
/// implement. The decoder never panics on malformed input.
pub fn decode_template(buf: &[u8]) -> Result<TemplateBundle<'_>> {
    container::decode_bundle(buf)
}
