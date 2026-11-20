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
pub use line::{
    FontMetrics, LineOptions, LineWhiteSpace, ResolvedFamily, ResolvedFont, ResolvedFontStyle,
    ShapedGlyph, ShapedLine, ShapedRun, Synthesis, font_metrics, shape_line,
};
/// The CSS generic font families, as parley names them: what
/// [`ResolvedFamily::Generic`] holds.
pub use parley::GenericFamily;
