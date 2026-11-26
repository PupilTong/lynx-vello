//! [`Raster`]: an `OffscreenCanvas`'s bitmap as a GPU texture, with the
//! draws its 2D context records into a display list and replays onto it.

use std::sync::Arc;

use seldon::text::ShapedLine;

use super::CompositeOp;
use super::context::State;
use super::style::{Paint, canvas_rect};
use crate::kurbo::{Affine, BezPath, Rect, Stroke};
use crate::paint::display_list::{self, DisplayList, draw_shaped_line};
use crate::peniko::{BlendMode, Extend, Fill, ImageQuality, ImageSampler, StyleRef};
use crate::render::canvas::{Init, MAX_SURFACE_SIDE, Surface, SurfaceImage};

const MAX_RECORDED_BYTES: usize = 2 << 20;

#[derive(Debug)]
pub(super) struct ClipNode {
    pub(super) path: BezPath,
    pub(super) rule: Fill,
    pub(super) depth: usize,
    pub(super) parent: Option<Arc<ClipNode>>,
}

pub(super) enum Outline<'a> {
    Fill(&'a BezPath, Fill),
    Stroke(&'a BezPath, &'a Stroke),
    Rect(Rect),
    Glyphs {
        line: &'a ShapedLine,
        stroke: Option<&'a Stroke>,
    },
}

#[derive(Debug)]
pub(in crate::html::canvas) struct Raster {
    pub(in crate::html::canvas) surface: Surface,
    pub(super) list: DisplayList,
    init: Init,
    keep: Vec<SurfaceImage>,
    clips: Vec<Arc<ClipNode>>,
    clip_scratch: Vec<Arc<ClipNode>>,
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

    pub(super) fn bound(&mut self) {
        if self.list.recorded_bytes() > MAX_RECORDED_BYTES {
            self.flush();
        }
    }

    pub(super) fn clear(&mut self) {
        self.list.reset();
        self.keep.clear();
        self.clips.clear();
        self.init = Init::Clear;
    }

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

    pub(super) fn sync_clips(&mut self, clip: Option<&Arc<ClipNode>>) {
        let depth = clip.map_or(0, |node| node.depth);
        if depth == self.clips.len()
            && clip.is_none_or(|node| Arc::ptr_eq(node, &self.clips[depth - 1]))
        {
            return;
        }
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
            self.flush();
        } else {
            self.bound();
        }
    }

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
                let style = match stroke {
                    Some(stroke) => StyleRef::Stroke(stroke),
                    None => StyleRef::Fill(Fill::NonZero),
                };
                draw_shaped_line(list, line, geometry, brush, brush_transform, style);
            }
        }
        if paint.bounds.is_some() {
            list.pop_layer();
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ShadowFilter {
    dx: f64,
    dy: f64,
    std_dev: f64,
}

const SHADOW_LAYER_SLACK: f64 = 2.0 * 6.0 + 16.0 + 1.0;

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
