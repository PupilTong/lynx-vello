//! [`Raster`]: an `OffscreenCanvas`'s bitmap as a GPU texture, with the
//! draws its 2D context records into a display list and replays onto it.

use std::sync::Arc;

use seldon::text::ShapedLine;

use super::CompositeOp;
use super::context::State;
use super::style::{Paint, canvas_rect};
use crate::kurbo::{Affine, BezPath, Rect, Stroke};
use crate::paint::display_list::{self, DisplayList, Glyph, synthesized_outlines};
use crate::peniko::{BlendMode, Extend, Fill, ImageQuality, ImageSampler};
use crate::render::canvas::{Init, MAX_SURFACE_SIDE, Surface, SurfaceImage};

/// How many bytes of draws are recorded before they are replayed with no
/// read asking: Chromium's bound on a canvas's recorded ops, 2 MiB.
const MAX_RECORDED_BYTES: usize = 2 << 20;

/// One clip path in canvas space, intersected with every clip before it.
#[derive(Debug)]
pub(super) struct ClipNode {
    pub(super) path: BezPath,
    pub(super) rule: Fill,
    /// Number of nodes on the chain, this one included.
    pub(super) depth: usize,
    pub(super) parent: Option<Arc<ClipNode>>,
}

/// What a draw outlines: a fill, a stroke, or glyphs.
pub(super) enum Outline<'a> {
    Fill(&'a BezPath, Fill),
    Stroke(&'a BezPath, &'a Stroke),
    Rect(Rect),
    Glyphs {
        line: &'a ShapedLine,
        stroke: Option<&'a Stroke>,
    },
}

/// The bitmap's texture and the draws recorded and not yet replayed onto
/// it.
///
/// Draws are recorded into `list`; [`Raster::flush`] hands it to the
/// surface, which replays it as `init` says. A batch of `source-over` draws
/// is replayed onto the bitmap as it is. A draw composited by any other
/// operation, or one that samples the canvas itself, needs the bitmap as
/// its backdrop: `vello_gpu` composites such a layer at the root into a
/// transparent intermediate and cannot sample the texture it renders into.
/// That batch starts with the bitmap drawn as its first item and is
/// rendered into the surface's second texture ([`Init::Backdrop`]). A batch
/// that starts by clearing the canvas holds all of its content and needs
/// neither.
#[derive(Debug)]
pub(in crate::html::canvas) struct Raster {
    pub(in crate::html::canvas) surface: Surface,
    pub(super) list: DisplayList,
    init: Init,
    /// The images `list` draws, kept until it is replayed.
    keep: Vec<SurfaceImage>,
    /// The clip paths open in `list`, outermost first.
    clips: Vec<Arc<ClipNode>>,
    clip_scratch: Vec<Arc<ClipNode>>,
    /// The canvas rectangle.
    pub(super) size: Rect,
}

impl Raster {
    pub(in crate::html::canvas) fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            surface: Surface::new(width, height)?,
            list: DisplayList::new(),
            init: Init::Load,
            keep: Vec::new(),
            clips: Vec::new(),
            clip_scratch: Vec::new(),
            size: canvas_rect(width, height),
        })
    }

    pub(in crate::html::canvas) fn pending(&self) -> bool {
        self.init != Init::Load || !self.list.is_empty()
    }

    /// Replays the recorded draws onto the bitmap.
    pub(in crate::html::canvas) fn flush(&mut self) {
        if !self.pending() {
            return;
        }
        for _ in self.clips.drain(..) {
            self.list.pop_layer();
        }
        self.surface.draw(&mut self.list, self.init, &mut self.keep);
        self.init = Init::Load;
    }

    /// Replays the recording when it has grown past [`MAX_RECORDED_BYTES`].
    pub(super) fn bound(&mut self) {
        if self.list.recorded_bytes() > MAX_RECORDED_BYTES {
            self.flush();
        }
    }

    /// Drops the recording: the bitmap is cleared to transparent black first
    /// at the next replay.
    pub(super) fn clear(&mut self) {
        self.list.reset();
        self.keep.clear();
        self.clips.clear();
        self.init = Init::Clear;
    }

    /// Makes the batch one that reads the bitmap: draws recorded so far are
    /// replayed, and the next batch starts with the bitmap drawn under the
    /// surface's own id.
    pub(super) fn ensure_backdrop(&mut self) {
        if self.init != Init::Load {
            return;
        }
        self.flush();
        self.init = Init::Backdrop;
        let own = display_list::Paint::texture(
            self.surface.texture(),
            ImageSampler::new()
                .with_extend(Extend::Pad)
                .with_quality(ImageQuality::Low),
        );
        self.list
            .fill_paint(Fill::NonZero, Affine::IDENTITY, own, None, &self.size);
    }

    /// Makes the clips open in the recording the chain ending at `clip`.
    pub(super) fn sync_clips(&mut self, clip: Option<&Arc<ClipNode>>) {
        let depth = clip.map_or(0, |node| node.depth);
        if depth == self.clips.len()
            && clip.is_none_or(|node| Arc::ptr_eq(node, &self.clips[depth - 1]))
        {
            return;
        }
        // Walk up from `clip` to the deepest node already open.
        self.clip_scratch.clear();
        let mut node = clip;
        let keep = loop {
            let Some(current) = node else { break 0 };
            if current.depth <= self.clips.len()
                && Arc::ptr_eq(current, &self.clips[current.depth - 1])
            {
                break current.depth;
            }
            self.clip_scratch.push(Arc::clone(current));
            node = current.parent.as_ref();
        };
        while self.clips.len() > keep {
            self.list.pop_layer();
            self.clips.pop();
        }
        while let Some(node) = self.clip_scratch.pop() {
            self.list
                .push_backdrop_clip(node.rule, Affine::IDENTITY, &node.path);
            self.clips.push(node);
        }
    }

    /// Records one draw (HTML "Drawing model"): the shadow, then the shape,
    /// each composited with the state's operation inside the clip.
    ///
    /// `geometry` maps the shape's coordinates to canvas space. `paint` of
    /// `None` paints transparent black, which still composites.
    pub(super) fn draw(
        &mut self,
        state: &State,
        paint: Option<&Paint>,
        geometry: Affine,
        shape: &Outline<'_>,
    ) {
        let op = state.composite;
        if op == CompositeOp::SourceOver && paint.is_none() {
            return;
        }
        let live = paint.and_then(|paint| paint.live);
        if op != CompositeOp::SourceOver
            || live.is_some_and(|texture| texture.id() == self.surface.texture().id())
        {
            self.ensure_backdrop();
        }
        let blend = op.blend();
        let visible = paint.filter(|_| geometry.determinant() != 0.0);
        if let Some(paint) = visible
            && state.casts_shadow()
            && let Some(filter) = shadow_filter(state, self.size)
        {
            self.shadow(state, blend, filter, paint, geometry, shape);
        }
        self.sync_clips(state.clip.as_ref());
        let layered = op != CompositeOp::SourceOver;
        if layered {
            self.list
                .push_layer(Fill::NonZero, blend, 1.0, Affine::IDENTITY, &self.size);
        }
        if let Some(paint) = visible {
            self.paint_shape(paint, geometry, shape);
        }
        if layered {
            self.list.pop_layer();
        }
        if let Some(keep) = paint.and_then(|paint| paint.keep.clone()) {
            self.keep.push(keep);
        }
        if live.is_some() {
            // The canvas it read may change before this one replays.
            self.flush();
        } else {
            self.bound();
        }
    }

    /// Records the shadow of a draw: cast by the unclipped shape, composited
    /// by `blend` inside the clip region.
    ///
    /// Clip paths applied to a layer's region cut what the layer composites
    /// and not what is drawn in it, while an open clip cuts both. Under
    /// `source-over` every clip path is a layer region of its own. Under
    /// another operation the blend has to read the bitmap, so the innermost
    /// path is the blend layer's region and the paths around it stay open
    /// clips, which cut the shape casting the shadow too.
    fn shadow(
        &mut self,
        state: &State,
        blend: BlendMode,
        filter: ShadowFilter,
        paint: &Paint,
        geometry: Affine,
        shape: &Outline<'_>,
    ) {
        let mut layers = 0;
        if state.composite == CompositeOp::SourceOver {
            self.sync_clips(None);
            let mut chain = Vec::new();
            let mut node = state.clip.as_ref();
            while let Some(current) = node {
                chain.push(Arc::clone(current));
                node = current.parent.as_ref();
            }
            for node in chain.iter().rev() {
                self.list.push_layer(
                    node.rule,
                    BlendMode::default(),
                    1.0,
                    Affine::IDENTITY,
                    &node.path,
                );
                layers += 1;
            }
        } else {
            let innermost = state.clip.as_ref();
            self.sync_clips(innermost.and_then(|node| node.parent.as_ref()));
            match innermost {
                Some(node) => {
                    self.list
                        .push_layer(node.rule, blend, 1.0, Affine::IDENTITY, &node.path);
                }
                None => {
                    self.list
                        .push_layer(Fill::NonZero, blend, 1.0, Affine::IDENTITY, &self.size);
                }
            }
            layers += 1;
        }
        self.list
            .push_drop_shadow(filter.dx, filter.dy, filter.std_dev, state.shadow_color);
        self.paint_shape(paint, geometry, shape);
        for _ in 0..=layers {
            self.list.pop_layer();
        }
    }

    fn paint_shape(&mut self, paint: &Paint, geometry: Affine, shape: &Outline<'_>) {
        let list = &mut self.list;
        if let Some(bounds) = &paint.bounds {
            list.push_backdrop_clip(Fill::NonZero, Affine::IDENTITY, bounds);
        }
        let brush = &paint.brush;
        let brush_transform = paint.brush_transform(geometry);
        match shape {
            Outline::Fill(path, rule) => {
                list.fill_paint(*rule, geometry, brush.clone(), brush_transform, *path);
            }
            Outline::Stroke(path, stroke) => {
                list.stroke_paint(stroke, geometry, brush.clone(), brush_transform, *path);
            }
            Outline::Rect(rect) => {
                list.fill_paint(
                    Fill::NonZero,
                    geometry,
                    brush.clone(),
                    brush_transform,
                    rect,
                );
            }
            Outline::Glyphs { line, stroke } => {
                draw_glyphs(list, line, *stroke, geometry, brush, brush_transform);
            }
        }
        if paint.bounds.is_some() {
            list.pop_layer();
        }
    }
}

/// A drop shadow as the filter draws it, in canvas px.
#[derive(Clone, Copy, Debug)]
struct ShadowFilter {
    dx: f64,
    dy: f64,
    std_dev: f64,
}

/// Px a shadow's filter layer leaves of the device's texture side
/// ([`MAX_SURFACE_SIDE`]): 6 of atlas padding on each side (`vello_gpu`'s
/// `FILTER_ATLAS_PADDING`, half its 13-tap kernel), 16 for rounding four
/// lengths up to the 4 px tile, and 1 for the `f32` rounding of the
/// filter's parameters.
const SHADOW_LAYER_SLACK: f64 = 2.0 * 6.0 + 16.0 + 1.0;

/// The shadow `state` casts on a canvas of `size`, its standard deviation
/// (half `shadowBlur`) limited to what `vello_gpu` can filter; `None` when
/// not even an unblurred shadow at its offsets can be.
///
/// `vello_gpu` renders a drop-shadow layer into an intermediate texture no
/// longer than the device's texture side, and a render with a layer past
/// it fails whole. Along each axis that layer (`vello_common`
/// `FilterLayerPlacement::new`) is at most the canvas side, plus the
/// source padding on both sides (`Filter::source_expansion`) and the
/// filter padding on one (`Filter::filter_expansion`), each rounded up to
/// the 4 px tile. For a reach R = 3σ and an offset of length a along the
/// axis that is under side + 2(a + R) + max(0, R − a) + 16 px, whatever the
/// shape, so σ is kept to the largest value for which that fits beside
/// the atlas padding. Browsers bound the blur too (Skia limits its σ).
fn shadow_filter(state: &State, size: Rect) -> Option<ShadowFilter> {
    let reach = |side: f64, offset: f64| {
        let budget = f64::from(MAX_SURFACE_SIDE) - SHADOW_LAYER_SLACK - side;
        let a = offset.abs();
        if 2.0 * a > budget {
            None
        } else if 4.0 * a <= budget {
            Some((budget - a) / 3.0)
        } else {
            Some(budget / 2.0 - a)
        }
    };
    let rx = reach(size.width(), state.shadow_offset_x)?;
    let ry = reach(size.height(), state.shadow_offset_y)?;
    Some(ShadowFilter {
        dx: state.shadow_offset_x,
        dy: state.shadow_offset_y,
        std_dev: (state.shadow_blur / 2.0).min(rx.min(ry) / 3.0),
    })
}

/// Records every glyph run of `line` under `geometry`, its alphabetic
/// baseline at y = 0, with the run's synthetic bold and oblique.
fn draw_glyphs(
    list: &mut DisplayList,
    line: &ShapedLine,
    stroke: Option<&Stroke>,
    geometry: Affine,
    brush: &display_list::Paint,
    brush_transform: Option<Affine>,
) {
    for run in &line.runs {
        let (embolden, glyph_transform) =
            synthesized_outlines(run.synthesis.embolden, run.synthesis.skew, run.size);
        let builder = list
            .draw_glyphs(&run.font)
            .font_size(run.size)
            .transform(geometry)
            .glyph_transform(glyph_transform)
            .font_embolden(embolden)
            .normalized_coords(&run.normalized_coords)
            .hint(false)
            .paint(brush.clone())
            .brush_transform(brush_transform);
        let glyphs = run.glyphs.iter().map(|glyph| Glyph {
            id: glyph.id,
            x: glyph.x,
            y: glyph.y,
        });
        match stroke {
            Some(stroke) => builder.draw(stroke, glyphs),
            None => builder.draw(Fill::NonZero, glyphs),
        }
    }
}
