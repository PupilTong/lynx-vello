//! Parley text layout: the Lynx text block and the context it shapes in.

pub mod block;
mod context;
mod font;
mod line;

pub use context::TextContext;
pub use font::FontBlob;
pub use line::{ResolvedFont, ResolvedFontStyle, ShapedGlyph, ShapedLine, ShapedRun, shape_line};
