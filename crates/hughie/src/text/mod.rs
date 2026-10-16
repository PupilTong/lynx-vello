//! Parley text layout: the shared paragraph core, the Lynx text block built
//! on it, and the context they shape in.

pub mod block;
mod context;
mod font;
mod line;
pub mod paragraph;

pub use context::TextContext;
pub use font::FontBlob;
pub use line::{ResolvedFont, ResolvedFontStyle, ShapedGlyph, ShapedLine, ShapedRun, shape_line};
