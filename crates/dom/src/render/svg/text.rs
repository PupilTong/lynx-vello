//! SVG `text`: the chunk model the parse collects and the shaper seam the
//! encode shapes it through.
//!
//! A `text` element is a list of *chunks*, each positioned absolutely
//! (the `text`'s own `x`/`y`, or a `tspan`'s), and a chunk is a list of
//! *spans*, one per run of characters under one style. `text-anchor`
//! anchors a whole chunk by its total advance, so a `tspan` that only
//! changes the font stays inside its chunk. Shaping needs fonts, which the
//! parse does not have: the chunks carry their text and resolved font, and
//! [`TextShaper`] shapes each span at encode time.

use std::sync::Arc;

use hughie::text::{ResolvedFont, ShapedLine, TextContext, shape_line};

use super::paint_server::GradientSpec;
use crate::vello::kurbo::{Affine, Stroke};
use crate::vello::peniko::Color;

/// Shapes one span of text; the document's `TextContext` behind a seam
/// the converter's tests can replace.
pub(crate) trait TextShaper {
    fn shape(&mut self, text: &str, font: &ResolvedFont, letter_spacing: f32) -> ShapedLine;
}

impl TextShaper for TextContext {
    fn shape(&mut self, text: &str, font: &ResolvedFont, letter_spacing: f32) -> ShapedLine {
        shape_line(self, text, font, letter_spacing)
    }
}

/// The document's shaper: the lazily created `TextContext` in its layout
/// state, created on the first span shaped, so a document that never
/// shows text and never draws SVG text pays for no font context.
pub(crate) struct DocumentShaper<'a>(pub(crate) &'a mut Option<Box<TextContext>>);

impl TextShaper for DocumentShaper<'_> {
    fn shape(&mut self, text: &str, font: &ResolvedFont, letter_spacing: f32) -> ShapedLine {
        let context = self.0.get_or_insert_with(|| Box::new(TextContext::new()));
        shape_line(context, text, font, letter_spacing)
    }
}

/// `text-anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum TextAnchor {
    #[default]
    Start,
    Middle,
    End,
}

/// One `text` element: its chunks, placed by the element's absolute
/// transform.
#[derive(Clone, Debug)]
pub(crate) struct TextItem {
    pub(crate) chunks: Vec<TextChunk>,
    pub(crate) transform: Affine,
}

/// A run of spans positioned together: the pen starts at `(x, y)` (the
/// alphabetic baseline) and the whole run is shifted by its anchor.
#[derive(Clone, Debug)]
pub(crate) struct TextChunk {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) anchor: TextAnchor,
    pub(crate) spans: Vec<TextSpan>,
}

/// Characters under one style. `dx`/`dy` move the pen before the span.
#[derive(Clone, Debug)]
pub(crate) struct TextSpan {
    pub(crate) text: String,
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    pub(crate) font: ResolvedFont,
    pub(crate) letter_spacing: f32,
    pub(crate) fill: Option<TextPaint>,
    pub(crate) stroke: Option<(Stroke, TextPaint)>,
    /// `paint-order`: the fill before the stroke.
    pub(crate) fill_first: bool,
}

/// A text paint, kept unresolved where it depends on the chunk's bounding
/// box, which only shaping knows.
#[derive(Clone, Debug)]
pub(crate) enum TextPaint {
    /// A colour with the paint's opacity folded in.
    Solid(Color),
    /// A gradient and the paint opacity to fold into its stops.
    Gradient(Arc<GradientSpec>, f32),
}

/// What a chunk's pen does: the whitespace rules of `xml:space`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Space {
    /// Newlines go, tabs become spaces, runs collapse to one, the
    /// element's ends are trimmed.
    Default,
    /// Newlines and tabs become spaces; nothing else changes.
    Preserve,
}

/// Normalises the text of every span of `chunks` in document order under
/// `space`, across span boundaries, and drops the spans and chunks left
/// empty.
pub(super) fn normalize_whitespace(chunks: &mut Vec<TextChunk>, space: Space) {
    let mut pending_space = false;
    let mut at_start = true;
    for chunk in chunks.iter_mut() {
        for span in &mut chunk.spans {
            let mut out = String::with_capacity(span.text.len());
            for character in span.text.chars() {
                match space {
                    Space::Preserve => {
                        out.push(if matches!(character, '\n' | '\r' | '\t') {
                            ' '
                        } else {
                            character
                        });
                    }
                    Space::Default => {
                        if matches!(character, '\n' | '\r') {
                            continue;
                        }
                        if character == ' ' || character == '\t' {
                            pending_space = !at_start;
                            continue;
                        }
                        if pending_space {
                            out.push(' ');
                            pending_space = false;
                        }
                        out.push(character);
                        at_start = false;
                    }
                }
            }
            span.text = out;
        }
    }
    // A pending space at the end is the trailing whitespace the default
    // mode trims, so it is simply never written.
    for chunk in chunks.iter_mut() {
        chunk.spans.retain(|span| !span.text.is_empty());
    }
    chunks.retain(|chunk| !chunk.spans.is_empty());
}
