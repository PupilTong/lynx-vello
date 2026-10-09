//! Items into a vello scene, text shaped on the way.

use hughie::text::ShapedLine;

use super::parse::opens_blend_in;
use super::text::{TextAnchor, TextItem, TextPaint, TextShaper};
use super::{Item, LayerClip, VectorDocument, paint_server};
use crate::vello::kurbo::{Affine, Point, Rect, Stroke};
use crate::vello::peniko::{BlendMode, Brush, Compose, Fill, Mix, StyleRef};
use crate::vello::{Glyph, Scene};

/// Encodes `document` into a scene in viewport units, shaping its text
/// through `shaper`. A document with nothing drawable encodes nothing.
pub(crate) fn encode(document: &VectorDocument, shaper: &mut dyn TextShaper) -> Scene {
    let mut scene = Scene::new();
    let shaped_texts = if document.has_text {
        shape_all(document, shaper)
    } else {
        Vec::new()
    };
    let extensions = layer_extensions(document, &shaped_texts);
    let mut texts = shaped_texts.iter();
    for (index, item) in document.items.iter().enumerate() {
        match item {
            Item::Path {
                shape,
                bounds: _,
                transform,
                fill,
                stroke,
                fill_first,
            } => {
                let fill = |scene: &mut Scene| {
                    if let Some(fill) = fill {
                        scene.fill(
                            fill.rule,
                            *transform,
                            &fill.brush,
                            fill.brush_transform,
                            shape,
                        );
                    }
                };
                let stroke = |scene: &mut Scene| {
                    if let Some(stroke) = stroke {
                        scene.stroke(
                            &stroke.style,
                            *transform,
                            &stroke.brush,
                            stroke.brush_transform,
                            shape,
                        );
                    }
                };
                if *fill_first {
                    fill(&mut scene);
                    stroke(&mut scene);
                } else {
                    stroke(&mut scene);
                    fill(&mut scene);
                }
            }
            Item::PushLayer {
                blend,
                alpha,
                clip,
                transform,
            } => match clip {
                LayerClip::Bounds(bounds) => {
                    let bounds = extend(*bounds, extensions.get(index).copied().flatten());
                    scene.push_layer(Fill::NonZero, *blend, *alpha, *transform, &bounds);
                }
                LayerClip::Path(shape, rule) => {
                    scene.push_layer(*rule, *blend, *alpha, *transform, shape);
                }
            },
            Item::PushClip {
                shape,
                rule,
                transform,
                isolate,
            } => {
                if *isolate {
                    scene.push_layer(
                        *rule,
                        BlendMode::new(Mix::Normal, Compose::SrcOver),
                        1.0,
                        *transform,
                        shape,
                    );
                } else {
                    scene.push_clip_layer(*rule, *transform, shape);
                }
            }
            Item::Pop => scene.pop_layer(),
            Item::Text(text) => {
                if let Some(shaped) = texts.next() {
                    draw_text(&mut scene, text, shaped);
                }
            }
        }
    }
    scene
}

/// Whether drawing `document` opens a blend layer with no isolating layer
/// of its own around it, so the layer a draw of the whole image opens must
/// be a full `Normal` layer rather than a clip layer (vello #1198).
pub(crate) fn opens_blend(document: &VectorDocument) -> bool {
    opens_blend_in(&document.items)
}

/// One text item's chunks, shaped and placed in the item's user space.
struct ShapedText {
    chunks: Vec<ShapedChunk>,
}

struct ShapedChunk {
    /// The chunk's bounding box in the item's user space: its advance
    /// between the fonts' ascent and descent.
    bbox: Rect,
    /// Each span's pen position and shaped line.
    spans: Vec<(Point, ShapedLine)>,
}

/// Shapes every text item of `document`, in item order.
fn shape_all(document: &VectorDocument, shaper: &mut dyn TextShaper) -> Vec<ShapedText> {
    document
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Text(text) => Some(text),
            _ => None,
        })
        .map(|text| ShapedText {
            chunks: text
                .chunks
                .iter()
                .map(|chunk| {
                    let lines: Vec<ShapedLine> = chunk
                        .spans
                        .iter()
                        .map(|span| shaper.shape(&span.text, &span.font, span.letter_spacing))
                        .collect();
                    // The pen runs from the chunk's start through every
                    // span's `dx` and advance; the anchor shifts the whole
                    // run by the distance it covered.
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
                .collect(),
        })
        .collect()
}

/// For every bounds-clipped compositing layer, the rectangle in its own
/// coordinates that the text drawn inside it reaches, which the parse
/// could not know. Indexed by item.
fn layer_extensions(document: &VectorDocument, shaped: &[ShapedText]) -> Vec<Option<Rect>> {
    if shaped.is_empty() {
        return Vec::new();
    }
    let mut extensions = vec![None; document.items.len()];
    // The open layers: item index and the inverse of the layer transform,
    // or `None` for a layer whose bounds text does not touch.
    let mut open: Vec<Option<(usize, Affine)>> = Vec::new();
    let mut texts = shaped.iter();
    for (index, item) in document.items.iter().enumerate() {
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
                let Some(shaped) = texts.next() else {
                    continue;
                };
                for (layer, inverse) in open.iter().flatten() {
                    let into = *inverse * text.transform;
                    for chunk in &shaped.chunks {
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

/// Draws one text item's shaped chunks as `paint/text.rs` draws a glyph
/// run: `draw_glyphs` with the run's font, size and normalised coordinates,
/// no hinting, the brush and its transform.
fn draw_text(scene: &mut Scene, text: &TextItem, shaped: &ShapedText) {
    for (chunk, placed) in text.chunks.iter().zip(&shaped.chunks) {
        for (span, (pen, line)) in chunk.spans.iter().zip(&placed.spans) {
            let run_transform = text.transform * Affine::translate(pen.to_vec2());
            // A gradient is defined in the item's user space; the run's
            // paint transform is relative to the run, so the pen is undone.
            let resolve = |paint: &TextPaint| -> Option<(Brush, Option<Affine>)> {
                match paint {
                    TextPaint::Solid(color) => Some((Brush::Solid(*color), None)),
                    TextPaint::Gradient(spec, opacity) => {
                        paint_server::brush(spec, Some(placed.bbox), *opacity).map(
                            |(brush, brush_transform)| {
                                (
                                    brush,
                                    brush_transform.map(|brush_transform| {
                                        Affine::translate(-pen.to_vec2()) * brush_transform
                                    }),
                                )
                            },
                        )
                    }
                }
            };
            let fill = span.fill.as_ref().and_then(resolve);
            let stroke = span
                .stroke
                .as_ref()
                .and_then(|(style, paint)| resolve(paint).map(|brush| (style, brush)));
            let draw = |scene: &mut Scene, style: StyleRef<'_>, brush: &(Brush, Option<Affine>)| {
                for run in &line.runs {
                    scene
                        .draw_glyphs(&run.font)
                        .font_size(run.size)
                        .transform(run_transform)
                        .normalized_coords(&run.normalized_coords)
                        .hint(false)
                        .brush(&brush.0)
                        .brush_transform(brush.1)
                        .draw(
                            style,
                            run.glyphs.iter().map(|glyph| Glyph {
                                id: glyph.id,
                                x: glyph.x,
                                y: glyph.y,
                            }),
                        );
                }
            };
            let draw_fill = |scene: &mut Scene| {
                if let Some(brush) = &fill {
                    draw(scene, Fill::NonZero.into(), brush);
                }
            };
            let draw_stroke = |scene: &mut Scene| {
                if let Some((style, brush)) = &stroke {
                    let style: &Stroke = style;
                    draw(scene, style.into(), brush);
                }
            };
            if span.fill_first {
                draw_fill(scene);
                draw_stroke(scene);
            } else {
                draw_stroke(scene);
                draw_fill(scene);
            }
        }
    }
}
