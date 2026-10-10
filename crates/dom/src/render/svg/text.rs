//! SVG `text`: the chunk model the parse collects and the encode shapes.
//!
//! A `text` element is a list of *chunks*, each positioned absolutely
//! (the `text`'s own `x`/`y`, or a `tspan`'s), and a chunk is a list of
//! *spans*, one per run of characters under one style. `text-anchor`
//! anchors a whole chunk by its total advance, so a `tspan` that only
//! changes the font stays inside its chunk. Shaping needs fonts, which the
//! parse does not have: the chunks carry their text and resolved font, and
//! the encode shapes each span through the document's `TextContext`
//! (`encode.rs`), created on the first span shaped so a document that never
//! draws text pays for no font context.

use std::sync::Arc;

use hughie::text::ResolvedFont;

use super::paint_server::GradientSpec;
use crate::vello::kurbo::{Affine, Stroke};
use crate::vello::peniko::Color;

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
///
/// A run of collapsed whitespace becomes one space, written once the next
/// character shows it is not trailing: before that character when it is in
/// the same chunk, else at the end of the span the run began in, because a
/// space belongs to the chunk that produced it (SVG 1.1 §10.15) and counts
/// in that chunk's advance, which `text-anchor` reads.
pub(super) fn normalize_whitespace(chunks: &mut Vec<TextChunk>, space: Space) {
    // Where the pending collapsed space began: its chunk and its span.
    let mut pending: Option<(usize, usize)> = None;
    let mut at_start = true;
    for chunk_index in 0..chunks.len() {
        for span_index in 0..chunks[chunk_index].spans.len() {
            let text = std::mem::take(&mut chunks[chunk_index].spans[span_index].text);
            let mut out = String::with_capacity(text.len());
            for character in text.chars() {
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
                            if !at_start && pending.is_none() {
                                pending = Some((chunk_index, span_index));
                            }
                            continue;
                        }
                        if let Some((produced_in, produced_by)) = pending.take() {
                            if produced_in == chunk_index {
                                out.push(' ');
                            } else {
                                chunks[produced_in].spans[produced_by].text.push(' ');
                            }
                        }
                        out.push(character);
                        at_start = false;
                    }
                }
            }
            chunks[chunk_index].spans[span_index].text = out;
        }
    }
    // A pending space at the end is the trailing whitespace the default
    // mode trims, so it is simply never written.
    for chunk in chunks.iter_mut() {
        chunk.spans.retain(|span| !span.text.is_empty());
    }
    chunks.retain(|chunk| !chunk.spans.is_empty());
}
