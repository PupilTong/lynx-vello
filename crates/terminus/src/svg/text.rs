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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum TextAnchor {
    #[default]
    Start,
    Middle,
    End,
}

#[derive(Clone, Debug)]
pub(crate) struct TextItem {
    pub(crate) node: crate::tree::document::NodeId,
    pub(crate) chunks: Vec<TextChunk>,
    pub(crate) transform: Affine,
    pub(crate) shaped: Vec<ShapedChunk>,
}

#[derive(Clone, Debug)]
pub(crate) struct ShapedChunk {
    pub(crate) bbox: Rect,
    pub(crate) spans: Vec<(Point, ShapedLine)>,
}

#[derive(Clone, Debug)]
pub(crate) struct TextChunk {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) anchor: TextAnchor,
    pub(crate) spans: Vec<TextSpan>,
}

#[derive(Clone, Debug)]
pub(crate) struct TextSpan {
    pub(crate) text: String,
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    pub(crate) font: ResolvedFont,
    pub(crate) letter_spacing: f32,
    pub(crate) fill: Option<TextPaint>,
    pub(crate) stroke: Option<(Stroke, TextPaint)>,
    pub(crate) fill_first: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum TextPaint {
    Solid(Color),
    Gradient(Arc<GradientSpec>, f32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Space {
    Default,
    Preserve,
}

pub(super) fn normalize_whitespace(chunks: &mut Vec<TextChunk>, space: Space) {
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
    for chunk in chunks.iter_mut() {
        chunk.spans.retain(|span| !span.text.is_empty());
    }
    chunks.retain(|chunk| !chunk.spans.is_empty());
}

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

fn layer_extensions(recording: &Recording) -> Vec<Option<Rect>> {
    let mut extensions = vec![None; recording.items.len()];
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

fn extend(bounds: Rect, extension: Option<Rect>) -> Rect {
    match extension {
        None => bounds,
        Some(extension) if bounds == Rect::ZERO => extension,
        Some(extension) => bounds.union(extension),
    }
}
