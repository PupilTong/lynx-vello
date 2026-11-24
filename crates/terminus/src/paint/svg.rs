//! The paint walk's SVG painter: one path or text item, or one layer push,
//! of an SVG image's recording (`crate::svg::present`), as the visual plan
//! placed it (`crate::visual::svg`).
//!
//! An item is drawn by [`draw`], under the transform the walk resolved for
//! the item in place of the item's own absolute one. A layer push is
//! described by [`layer`] and opened under the same kind of transform: the
//! monolithic walk pushes it into its display list with [`Layer::push`],
//! and the compose walk emits it as a `ComposeOp::Push` of its rectangle or
//! path (`paint::walker`, `push_svg_layer`). Glyphs are drawn as
//! `paint/text.rs` draws them (`draw_glyphs`, the run's size and normalised
//! coordinates, no hinting). A paint item's and a layer's
//! matrix place the item's ink rectangle or the layer shape's bounding box
//! at its origin, so the painter moves the origin back before it draws.

use std::sync::Arc;

use crate::kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use crate::paint::display_list::{DisplayList, Glyph, synthesized_outlines};
use crate::peniko::{Brush, Fill, Mix, StyleRef};
use crate::svg::recording::{FillPaint, Item, LayerClip, StrokePaint};
use crate::svg::text::{TextItem, TextPaint};
use crate::svg::{ink_extent, paint_server};
use crate::tree::document::{Document, NodeId};

/// Item `index` of `root`'s recording, if it is still recorded.
fn recorded<T>(document: &Document<T>, root: NodeId, index: u32) -> Option<&Item> {
    document
        .svg_recording(root)?
        .items
        .get(usize::try_from(index).ok()?)
}

/// Draws path or text item `index` of `root`'s recording into `scene`,
/// `transform` mapping its paint item's box to device px, and answers
/// whether it drew.
pub(super) fn paint_item<T>(
    scene: &mut DisplayList,
    document: &Document<T>,
    (root, index): (NodeId, u32),
    transform: Affine,
) -> bool {
    let Some(item) = recorded(document, root, index) else {
        return false;
    };
    let Some(extent) = ink_extent(item) else {
        return false;
    };
    draw(
        scene,
        item,
        transform * Affine::translate(-extent.origin().to_vec2()),
    );
    true
}

/// The layer push `index` of `root`'s recording and the transform it is
/// pushed under, `transform` mapping its group layer's box to device px.
pub(super) fn layer_push<T>(
    document: &Document<T>,
    (root, index): (NodeId, u32),
    transform: Affine,
) -> Option<(Layer<'_>, Affine)> {
    let pushed = layer(recorded(document, root, index)?)?;
    let origin = pushed.shape.bounds().origin();
    Some((pushed, transform * Affine::translate(-origin.to_vec2())))
}

/// Whether item `index` of `root`'s recording is text, whose glyph outlines
/// can leave the item's box.
pub(super) fn is_text<T>(document: &Document<T>, (root, index): (NodeId, u32)) -> bool {
    matches!(recorded(document, root, index), Some(Item::Text(_)))
}

/// Draws one path or text item under `transform`, which stands for the
/// item's own absolute transform: the paint walk passes the item's place
/// on the page or in the image. A layer push or a pop draws nothing here.
/// Its text is drawn as [`shape`](crate::svg::text::shape) left it: a text
/// item that was never shaped draws nothing.
fn draw(scene: &mut DisplayList, item: &Item, transform: Affine) {
    match item {
        Item::Path {
            shape,
            fill,
            stroke,
            fill_first,
            ..
        } => draw_path(
            scene,
            shape,
            fill.as_ref(),
            stroke.as_ref(),
            *fill_first,
            transform,
        ),
        Item::Text(text) => draw_text(scene, text, transform),
        Item::PushLayer { .. } | Item::PushClip { .. } | Item::Pop => {}
    }
}

fn draw_path(
    scene: &mut DisplayList,
    shape: &BezPath,
    fill: Option<&FillPaint>,
    stroke: Option<&StrokePaint>,
    fill_first: bool,
    transform: Affine,
) {
    let fill = |scene: &mut DisplayList| {
        if let Some(fill) = fill {
            scene.fill(
                fill.rule,
                transform,
                &fill.brush,
                fill.brush_transform,
                shape,
            );
        }
    };
    let stroke = |scene: &mut DisplayList| {
        if let Some(stroke) = stroke {
            scene.stroke(
                &stroke.style,
                transform,
                &stroke.brush,
                stroke.brush_transform,
                shape,
            );
        }
    };
    if fill_first {
        fill(scene);
        stroke(scene);
    } else {
        stroke(scene);
        fill(scene);
    }
}

/// One layer push of a recording: a `Normal` compositing layer at `alpha`,
/// or a clip, shaped by a rectangle or a path under a fill rule.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layer<'a> {
    pub(crate) clip_only: bool,
    pub(crate) alpha: f32,
    pub(crate) rule: Fill,
    /// The layer's absolute transform, in viewport units.
    pub(crate) transform: Affine,
    pub(crate) shape: LayerShape<'a>,
}

/// What a [`Layer`] is shaped by, in the coordinates its transform maps
/// from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum LayerShape<'a> {
    Rect(Rect),
    Path(&'a Arc<BezPath>),
}

impl LayerShape<'_> {
    /// The shape's bounding box in its own coordinates.
    pub(crate) fn bounds(self) -> Rect {
        match self {
            Self::Rect(rect) => rect,
            Self::Path(path) => path.bounding_box(),
        }
    }
}

impl Layer<'_> {
    /// Pushes this layer under `transform`, which stands for its own
    /// transform as in [`draw`].
    pub(crate) fn push(&self, scene: &mut DisplayList, transform: Affine) {
        match (self.clip_only, self.shape) {
            (true, LayerShape::Rect(rect)) => scene.push_clip_layer(self.rule, transform, &rect),
            (true, LayerShape::Path(path)) => {
                scene.push_clip_layer(self.rule, transform, &**path);
            }
            (false, LayerShape::Rect(rect)) => {
                scene.push_layer(self.rule, Mix::Normal, self.alpha, transform, &rect);
            }
            (false, LayerShape::Path(path)) => {
                scene.push_layer(self.rule, Mix::Normal, self.alpha, transform, &**path);
            }
        }
    }
}

/// The layer `item` pushes, if it is a layer push. A bounds layer is shaped
/// by its bounds under `nonzero`.
pub(crate) fn layer(item: &Item) -> Option<Layer<'_>> {
    match item {
        Item::PushLayer {
            alpha,
            clip,
            transform,
            ..
        } => {
            let (rule, shape) = match clip {
                LayerClip::Bounds(bounds) => (Fill::NonZero, LayerShape::Rect(*bounds)),
                LayerClip::Path(shape, rule) => (*rule, LayerShape::Path(shape)),
            };
            Some(Layer {
                clip_only: false,
                alpha: *alpha,
                rule,
                transform: *transform,
                shape,
            })
        }
        Item::PushClip {
            shape,
            rule,
            transform,
            ..
        } => Some(Layer {
            clip_only: true,
            alpha: 1.0,
            rule: *rule,
            transform: *transform,
            shape: LayerShape::Path(shape),
        }),
        Item::Path { .. } | Item::Pop | Item::Text(_) => None,
    }
}

/// Draws one text item's shaped chunks under `transform` as `paint/text.rs`
/// draws a glyph run: `draw_glyphs` with the run's font, size and
/// normalised coordinates, no hinting, the brush and its transform.
fn draw_text(scene: &mut DisplayList, text: &TextItem, transform: Affine) {
    for (chunk, placed) in text.chunks.iter().zip(&text.shaped) {
        for (span, (pen, line)) in chunk.spans.iter().zip(&placed.spans) {
            let run_transform = transform * Affine::translate(pen.to_vec2());
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
            let draw = |scene: &mut DisplayList,
                        style: StyleRef<'_>,
                        brush: &(Brush, Option<Affine>)| {
                for run in &line.runs {
                    let (embolden, glyph_transform) =
                        synthesized_outlines(run.synthesis.embolden, run.synthesis.skew, run.size);
                    scene
                        .draw_glyphs(&run.font)
                        .font_size(run.size)
                        .transform(run_transform)
                        .glyph_transform(glyph_transform)
                        .font_embolden(embolden)
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
            let draw_fill = |scene: &mut DisplayList| {
                if let Some(brush) = &fill {
                    draw(scene, Fill::NonZero.into(), brush);
                }
            };
            let draw_stroke = |scene: &mut DisplayList| {
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
