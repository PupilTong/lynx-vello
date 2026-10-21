//! Parley text layout: the shared paragraph core, the two paragraph kinds
//! built on it — the Lynx text block and a CSS block container's inline
//! formatting context — and the context they shape in.

pub mod block;
mod context;
mod font;
pub mod inline;
mod line;
pub mod paragraph;

pub use context::TextContext;
pub use font::FontBlob;
pub use line::{ResolvedFont, ResolvedFontStyle, ShapedGlyph, ShapedLine, ShapedRun, shape_line};
