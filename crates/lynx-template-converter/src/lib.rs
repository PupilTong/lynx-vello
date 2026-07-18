//! Native-to-web conversion for Lynx **source-based external bundles**.
//!
//! Lynx native cards and production external bundles may contain `QuickJS`
//! bytecode. The web `SDRA WROF` format, however, requires JavaScript source.
//! Conversion is therefore deliberately limited to flexible native external
//! bundles whose custom sections still contain source text. Any root Lepus,
//! Lepus-chunk, `JS_BYTECODE`, or `JsBytecode` custom section returns
//! [`ConvertError::CodeCacheBundle`] before an output bundle is produced.
//!
//! # Example
//!
//! ```no_run
//! let native = std::fs::read("library.lynx.bundle").unwrap();
//! let web = lynx_template_converter::convert(&native).unwrap();
//! std::fs::write("library.web.bundle", web).unwrap();
//! ```

mod error;
mod native;
mod reader;
mod style;
mod tokenize;
mod web;

pub use error::ConvertError;

/// Converts a source-based Lynx native external bundle to a web binary
/// template (`SDRA WROF`).
///
/// # Errors
///
/// Returns [`ConvertError::CodeCacheBundle`] when any executable section is
/// bytecode-only. Other errors describe malformed or currently unsupported
/// native encodings.
pub fn convert(native_bundle: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let native = native::decode(native_bundle)?;
    web::encode(native)
}
