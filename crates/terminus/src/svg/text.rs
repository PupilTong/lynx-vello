//! SVG `text`: the chunk model the recorder collects, the whitespace rule
//! `xml:space` applies to it, and the shaping that turns it into glyph runs.
//!
//! A `text` element is a list of *chunks*, each positioned absolutely
//! (the `text`'s own `x`/`y`, or a `tspan`'s), and a chunk is a list of
//! *spans*, one per run of characters under one style. `text-anchor`
//! anchors a whole chunk by its total advance, so a `tspan` that only
//! changes the font stays inside its chunk. Shaping needs fonts, which the
//! walk over the cascaded tree does not read: the chunks carry their text
//! and resolved font, and [`shape`] shapes each span through the document's
//! `TextContext`, created on the first span shaped so a document that never
//! draws text pays for no font context, and stores the result in the item
//! ([`TextItem::shaped`]). A gradient paint resolves against the chunk's
//! advance box once shaped, and text inside a compositing layer extends the
//! layer's bounds when it is shaped. A `@font-face` registered after a
//! recording was shaped does not re-shape it.

use std::sync::Arc;

use seldon::text::{LineOptions, ResolvedFont, ShapedLine, TextContext, shape_line};

use super::paint_server::GradientSpec;
use super::recording::{Item, LayerClip, Recording};
use crate::kurbo::{Affine, Point, Rect, Stroke};
use crate::peniko::Color;

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
    /// The `text` element.
    pub(crate) node: crate::tree::document::NodeId,
    pub(crate) chunks: Vec<TextChunk>,
    pub(crate) transform: Affine,
    /// Each chunk shaped, index-parallel with `chunks`; empty until
    /// [`shape`] runs, and a text item with no shaped chunks
    /// draws nothing.
    pub(crate) shaped: Vec<ShapedChunk>,
}

/// One chunk shaped and placed in the item's user space.
#[derive(Clone, Debug)]
pub(crate) struct ShapedChunk {
    /// The chunk's bounding box in the item's user space: its advance
    /// between the fonts' ascent and descent.
    pub(crate) bbox: Rect,
    /// Each span's pen position and shaped line.
    pub(crate) spans: Vec<(Point, ShapedLine)>,
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

/// Shapes every text item of `recording` that is not shaped yet through
/// the document's `context`, created here on the first recording with
/// text so one without pays for no font context, and grows every
/// bounds-clipped compositing layer by the text drawn inside it. Shaping a
/// recording twice changes nothing.
pub(crate) fn shape(recording: &mut Recording, context: &mut Option<Box<TextContext>>) {
    if !recording.has_text {
        return;
    }
    let context = context.get_or_insert_with(|| Box::new(TextContext::new()));
    for item in &mut recording.items {
        if let Item::Text(text) = item
            && text.shaped.is_empty()
        {
            text.shaped = shape_text(text, context);
        }
    }
    let extensions = layer_extensions(recording);
    for (item, extension) in recording.items.iter_mut().zip(extensions) {
        if let Item::PushLayer {
            clip: LayerClip::Bounds(bounds),
            ..
        } = item
        {
            *bounds = extend(*bounds, extension);
        }
    }
}

/// `text`'s chunks shaped and placed in the item's user space.
fn shape_text(text: &TextItem, context: &mut TextContext) -> Vec<ShapedChunk> {
    text.chunks
        .iter()
        .map(|chunk| {
            let lines: Vec<ShapedLine> = chunk
                .spans
                .iter()
                .map(|span| {
                    let options = LineOptions {
                        letter_spacing: span.letter_spacing,
                        ..LineOptions::default()
                    };
                    shape_line(context, &span.text, &span.font, options)
                })
                .collect();
            // The pen runs from the chunk's start through every span's `dx`
            // and advance; the anchor shifts the whole run by the distance
            // it covered.
            let width: f32 = chunk
                .spans
                .iter()
                .zip(&lines)
                .map(|(span, line)| span.dx + line.advance)
                .sum();
            let shift = match chunk.anchor {
                TextAnchor::Start => 0.0,
                TextAnchor::Middle => -width / 2.0,
                TextAnchor::End => -width,
            };
            let mut pen = Point::new(f64::from(chunk.x + shift), f64::from(chunk.y));
            let mut bbox: Option<Rect> = None;
            let spans = chunk
                .spans
                .iter()
                .zip(lines)
                .map(|(span, line)| {
                    pen += (f64::from(span.dx), f64::from(span.dy));
                    let span_box = Rect::new(
                        pen.x,
                        pen.y - f64::from(line.ascent),
                        pen.x + f64::from(line.advance),
                        pen.y + f64::from(line.descent),
                    );
                    bbox = Some(bbox.map_or(span_box, |bbox| bbox.union(span_box)));
                    let placed = (pen, line);
                    pen.x += f64::from(placed.1.advance);
                    placed
                })
                .collect();
            ShapedChunk {
                bbox: bbox.unwrap_or(Rect::ZERO),
                spans,
            }
        })
        .collect()
}

/// For every bounds-clipped compositing layer, the rectangle in its own
/// coordinates that the shaped text drawn inside it reaches, which the
/// recording could not know. Indexed by item.
fn layer_extensions(recording: &Recording) -> Vec<Option<Rect>> {
    let mut extensions = vec![None; recording.items.len()];
    // The open layers: item index and the inverse of the layer transform,
    // or `None` for a layer whose bounds text does not touch.
    let mut open: Vec<Option<(usize, Affine)>> = Vec::new();
    for (index, item) in recording.items.iter().enumerate() {
        match item {
            Item::PushLayer {
                clip: LayerClip::Bounds(_),
                transform,
                ..
            } => open.push(
                (transform.determinant().abs() >= f64::EPSILON)
                    .then(|| (index, transform.inverse())),
            ),
            Item::PushLayer { .. } | Item::PushClip { .. } => open.push(None),
            Item::Pop => {
                open.pop();
            }
            Item::Path { .. } => {}
            Item::Text(text) => {
                for (layer, inverse) in open.iter().flatten() {
                    let into = *inverse * text.transform;
                    for chunk in &text.shaped {
                        let reach = into.transform_rect_bbox(chunk.bbox);
                        let slot: &mut Option<Rect> = &mut extensions[*layer];
                        *slot = Some(slot.map_or(reach, |current| current.union(reach)));
                    }
                }
            }
        }
    }
    extensions
}

/// `bounds` grown by `extension`; a layer with no path content takes the
/// text's extent alone rather than a box that also holds the origin.
fn extend(bounds: Rect, extension: Option<Rect>) -> Rect {
    match extension {
        None => bounds,
        Some(extension) if bounds == Rect::ZERO => extension,
        Some(extension) => bounds.union(extension),
    }
}
