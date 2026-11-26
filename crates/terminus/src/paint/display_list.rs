//! The display list: what the paint walk records, what a committed frame
//! retains, and what the rasterizer replays (`render::sparse`).
//!
//! It holds drawing semantics, not a bit stream. A fill or stroke keeps its
//! shape as the shape it was recorded as — a rectangle stays a [`Rect`], a
//! rounded rectangle a [`RoundedRect`], anything else a path — together with
//! its transform, its paint and the paint's own transform. A clip keeps
//! whether it has to stay an isolated layer ([`Item::PushClip`]); a layer
//! keeps its blend mode and alpha; a glyph run keeps its font, size,
//! transforms and synthetic bold amount ([`GlyphRun::embolden`]); an image
//! keeps its pixels' handle
//! and sampler, a baked texture its id ([`Texture`]); a gradient is shared by
//! reference, so a list that copies it ([`DisplayList::append`]) copies no
//! stops.
//!
//! Transforms, rounded-rectangle radii, paths, glyphs and variation
//! coordinates live in arenas the items index, a run of items under one
//! transform sharing its entry. That keeps an item small, and appending one
//! list to another — the compose program's per-frame assembly of a committed
//! frame's fragments — copies items and arena slices, maps each distinct
//! transform once, and allocates nothing per item.
//!
//! The recording calls mirror the drawing model the paint modules were
//! written against: shapes under a transform, a layer stack of clips and
//! group layers, and glyph runs built by a [`DrawGlyphs`] builder. Every
//! clip and layer pushed is popped by the matching [`DisplayList::pop_layer`].
//!
//! A list the compose program retains as a fragment is sealed
//! ([`DisplayList::seal_bounds`]): its items are cut into chunks of a few
//! items each, every cut where no clip or layer is open, and each chunk
//! keeps a bound on where its items can put ink in the list's space. The
//! rasterizer turns every path it is given into strips, whether or not it
//! lands on the target, so the per-frame assembly appends only the chunks
//! whose bound, under the fragment's compose transform, reaches what the
//! target shows ([`DisplayList::append_visible`]). A chunk opens and closes
//! its own clips and layers, so leaving one out leaves the layer stack
//! balanced.

use std::ops::Range;
use std::sync::Arc;

pub use glifo::Glyph;
use seldon::text::ShapedLine;
use smallvec::SmallVec;

use crate::kurbo::{Affine, Join, PathEl, Rect, RoundedRect, RoundedRectRadii, Stroke};
use crate::peniko::{
    BlendMode, Blob, Brush, BrushRef, Color, Compose, Fill, FontData, Gradient, ImageBrushRef,
    ImageData, ImageSampler, StyleRef, WeakBlob,
};
use crate::render::image::ImageDescriptor;
use crate::render::picture::PictureId;

pub type NormalizedCoord = i16;

pub(crate) const FILL_TOLERANCE: f64 = 0.1;

pub(crate) const STROKE_TOLERANCE: f64 = 0.01;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Shape {
    Rect(Rect),
    RoundedRect { rect: Rect, radii: u32 },
    Path { start: u32, end: u32 },
}

const NO_TRANSFORM: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    Solid(Color),
    Gradient(Arc<Gradient>),
    Image(Arc<ImagePaint>),
    Texture(Arc<TexturePaint>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TexturePaint {
    pub(crate) texture: Texture,
    pub(crate) sampler: ImageSampler,
}

impl Paint {
    pub(crate) fn texture(texture: Texture, sampler: ImageSampler) -> Self {
        Self::Texture(Arc::new(TexturePaint { texture, sampler }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texture {
    id: u64,
    width: u32,
    height: u32,
    stored_width: u32,
    stored_height: u32,
}

impl Texture {
    pub(crate) fn new(width: u32, height: u32, texture_width: u32, texture_height: u32) -> Self {
        Self {
            id: next_texture_id(),
            width,
            height,
            stored_width: texture_width,
            stored_height: texture_height,
        }
    }

    pub(crate) fn with_id(id: u64, width: u32, height: u32) -> Self {
        Self {
            id,
            width,
            height,
            stored_width: width,
            stored_height: height,
        }
    }

    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn texture_size(&self) -> (u32, u32) {
        (self.stored_width, self.stored_height)
    }
}

pub(crate) fn next_texture_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Debug)]
pub(crate) struct ImagePaint {
    pub(crate) image: ImageRef,
    pub(crate) sampler: ImageSampler,
}

impl PartialEq for ImagePaint {
    fn eq(&self, other: &Self) -> bool {
        self.image.descriptor == other.image.descriptor
            && self.image.picture == other.image.picture
            && self.sampler == other.sampler
    }
}

#[derive(Clone, Debug)]
pub struct ImageRef {
    pub descriptor: ImageDescriptor,
    pub pixels: ImagePixels,
    pub picture: Option<PictureId>,
}

#[derive(Clone, Debug)]
pub enum ImagePixels {
    Held(Blob<u8>),
    Lent(WeakBlob<u8>),
    Uploaded,
}

impl ImageRef {
    #[must_use]
    pub fn held(image: &ImageData) -> Self {
        Self {
            descriptor: ImageDescriptor::of(image),
            pixels: ImagePixels::Held(image.data.clone()),
            picture: None,
        }
    }

    #[must_use]
    pub fn lent(image: &ImageData) -> Self {
        Self {
            descriptor: ImageDescriptor::of(image),
            pixels: ImagePixels::Lent(image.data.downgrade()),
            picture: None,
        }
    }

    #[must_use]
    pub fn uploaded(descriptor: ImageDescriptor) -> Self {
        Self {
            descriptor,
            pixels: ImagePixels::Uploaded,
            picture: None,
        }
    }

    #[must_use]
    pub fn written(picture: PictureId, descriptor: ImageDescriptor) -> Self {
        Self {
            descriptor,
            pixels: ImagePixels::Uploaded,
            picture: Some(picture),
        }
    }

    #[must_use]
    pub fn pixels(&self) -> Option<ImageData> {
        let data = match &self.pixels {
            ImagePixels::Held(data) => data.clone(),
            ImagePixels::Lent(weak) => weak.upgrade()?,
            ImagePixels::Uploaded => return None,
        };
        Some(self.descriptor.with_pixels(data))
    }
}

impl Paint {
    fn multiply_alpha(self, alpha: f32) -> Self {
        #[expect(
            clippy::float_cmp,
            reason = "1 is exactly the alpha that changes nothing"
        )]
        if alpha == 1.0 {
            return self;
        }
        match self {
            Self::Solid(color) => Self::Solid(color.multiply_alpha(alpha)),
            Self::Gradient(gradient) => {
                Self::Gradient(Arc::new((*gradient).clone().multiply_alpha(alpha)))
            }
            Self::Image(image) => Self::Image(Arc::new(ImagePaint {
                image: image.image.clone(),
                sampler: image.sampler.multiply_alpha(alpha),
            })),
            Self::Texture(paint) => {
                Self::texture(paint.texture, paint.sampler.multiply_alpha(alpha))
            }
        }
    }
}

impl From<BrushRef<'_>> for Paint {
    fn from(brush: BrushRef<'_>) -> Self {
        match brush {
            BrushRef::Solid(color) => Self::Solid(color),
            BrushRef::Gradient(gradient) => gradient_paint(gradient),
            BrushRef::Image(image) => Self::Image(Arc::new(ImagePaint {
                image: ImageRef::held(image.image),
                sampler: image.sampler,
            })),
        }
    }
}

impl From<&Brush> for Paint {
    fn from(brush: &Brush) -> Self {
        BrushRef::from(brush).into()
    }
}

fn gradient_paint(gradient: &Gradient) -> Paint {
    match gradient.stops.as_slice() {
        [] => Paint::Solid(Color::TRANSPARENT),
        [only] => Paint::Solid(only.color.to_alpha_color()),
        _ => Paint::Gradient(Arc::new(gradient.clone())),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Draw {
    pub(crate) rule: Fill,
    pub(crate) transform: u32,
    pub(crate) paint_transform: u32,
    pub(crate) shape: Shape,
    pub(crate) paint: Paint,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Region {
    pub(crate) rule: Fill,
    pub(crate) transform: u32,
    pub(crate) shape: Shape,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BlurredRoundedRect {
    pub(crate) transform: u32,
    pub(crate) rect: Rect,
    pub(crate) color: Color,
    pub(crate) radius: f32,
    pub(crate) std_dev: f32,
}

#[derive(Clone, Debug)]
pub(crate) enum Item {
    Fill(Draw),
    Stroke(Draw, Arc<Stroke>),
    PushClip {
        region: Region,
        isolated: bool,
        isolable: bool,
    },
    PushLayer {
        region: Region,
        blend: BlendMode,
        alpha: f32,
    },
    PushShadow(DropShadow),
    Pop,
    Glyphs(u32),
    BlurredRoundedRect(BlurredRoundedRect),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DropShadow {
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    pub(crate) std_dev: f32,
    pub(crate) color: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GlyphStyle {
    Fill,
    Stroke(Arc<Stroke>),
}

pub(crate) fn synthesized_outlines(
    embolden: bool,
    skew: Option<f32>,
    font_size: f32,
) -> (Embolden, Option<Affine>) {
    let embolden = if embolden {
        Embolden::uniform(f64::from(font_size) / 48.0)
    } else {
        Embolden::default()
    };
    let glyph_transform =
        skew.map(|degrees| Affine::skew(-f64::from(degrees).to_radians().tan(), 0.0));
    (embolden, glyph_transform)
}

pub(crate) fn draw_shaped_line(
    list: &mut DisplayList,
    line: &ShapedLine,
    transform: Affine,
    paint: &Paint,
    paint_transform: Option<Affine>,
    style: StyleRef<'_>,
) {
    for run in &line.runs {
        let (embolden, glyph_transform) =
            synthesized_outlines(run.synthesis.embolden, run.synthesis.skew, run.size);
        list.draw_glyphs(&run.font)
            .font_size(run.size)
            .transform(transform)
            .glyph_transform(glyph_transform)
            .font_embolden(embolden)
            .normalized_coords(&run.normalized_coords)
            .hint(false)
            .paint(paint.clone())
            .brush_transform(paint_transform)
            .draw(
                style,
                run.glyphs.iter().map(|glyph| Glyph {
                    id: glyph.id,
                    x: glyph.x,
                    y: glyph.y,
                }),
            );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Embolden {
    pub x: f64,
    pub y: f64,
    pub join: crate::kurbo::Join,
    pub miter_limit: f64,
    pub tolerance: f64,
}

impl Default for Embolden {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            join: crate::kurbo::Join::Miter,
            miter_limit: 4.0,
            tolerance: 0.1,
        }
    }
}

impl Embolden {
    #[must_use]
    pub fn uniform(amount: f64) -> Self {
        Self {
            x: amount,
            y: amount,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GlyphRun {
    pub(crate) font: FontData,
    pub(crate) font_size: f32,
    pub(crate) transform: Affine,
    pub(crate) glyph_transform: Option<Affine>,
    pub(crate) paint: Paint,
    pub(crate) paint_transform: Option<Affine>,
    pub(crate) style: GlyphStyle,
    pub(crate) hint: bool,
    pub(crate) embolden: Embolden,
    pub(crate) glyphs: Range<u32>,
    pub(crate) coords: Range<u32>,
}

#[derive(Default)]
pub struct DisplayList {
    items: Vec<Item>,
    transforms: Vec<Affine>,
    paint_transforms: Vec<Affine>,
    radii: Vec<RoundedRectRadii>,
    paths: Vec<PathEl>,
    runs: Vec<GlyphRun>,
    glyphs: Vec<Glyph>,
    coords: Vec<NormalizedCoord>,
    open: Vec<u32>,
    blends_at_root: bool,
    chunks: Vec<Chunk>,
    ink: Rect,
}

#[derive(Clone, Copy, Debug)]
struct Chunk {
    end: u32,
    bounds: Rect,
}

const CHUNK_ITEMS: usize = 4;

const NO_INK: Rect = Rect::new(
    f64::INFINITY,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NEG_INFINITY,
);

const ANYWHERE: Rect = Rect::new(
    f64::NEG_INFINITY,
    f64::NEG_INFINITY,
    f64::INFINITY,
    f64::INFINITY,
);

fn grow(acc: &mut Rect, ink: Rect) {
    if ink.x0.is_nan() || ink.y0.is_nan() || ink.x1.is_nan() || ink.y1.is_nan() {
        *acc = ANYWHERE;
    } else if ink.x1 > ink.x0 && ink.y1 > ink.y0 {
        *acc = acc.union(ink);
    }
}

fn cut(ink: Rect, clip: Option<Rect>) -> Rect {
    match clip {
        Some(clip) if ink.x0.is_nan() || ink.y0.is_nan() || ink.x1.is_nan() || ink.y1.is_nan() => {
            clip
        }
        Some(clip) => ink.intersect(clip),
        None => ink,
    }
}

pub(crate) fn may_reach(bounds: Rect, transform: Affine, visible: Rect) -> bool {
    if bounds.x0 > bounds.x1 || bounds.y0 > bounds.y1 {
        return false;
    }
    if !bounds.is_finite() || !transform.is_finite() {
        return true;
    }
    let mapped = transform.transform_rect_bbox(bounds);
    !(mapped.x1 < visible.x0
        || mapped.x0 > visible.x1
        || mapped.y1 < visible.y0
        || mapped.y0 > visible.y1)
}

fn close_chunk(chunks: &mut Vec<Chunk>, ink: &mut Rect, end: usize, bounds: Rect) {
    chunks.push(Chunk {
        end: index(end),
        bounds,
    });
    if bounds.x0 <= bounds.x1 && bounds.y0 <= bounds.y1 {
        *ink = if bounds.is_finite() {
            ink.union(bounds)
        } else {
            ANYWHERE
        };
    }
}

fn stroke_reach(stroke: &Stroke) -> f64 {
    let half = stroke.width.abs() / 2.0;
    let join = if stroke.join == Join::Miter {
        stroke.miter_limit.max(1.0)
    } else {
        1.0
    };
    half * join.max(std::f64::consts::SQRT_2)
}

fn clears_under_empty_source(blend: BlendMode) -> bool {
    matches!(
        blend.compose,
        Compose::Clear
            | Compose::Copy
            | Compose::SrcIn
            | Compose::DestIn
            | Compose::SrcOut
            | Compose::DestAtop
    )
}

const GLYPH_MARGIN_EM: f64 = 1.0;

const GLYPH_FLOOR_EM: Rect = Rect::new(-1.0, -2.0, 2.0, 1.0);

impl std::fmt::Debug for DisplayList {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DisplayList")
            .field("items", &self.items.len())
            .field("open", &self.open.len())
            .finish_non_exhaustive()
    }
}

impl Clone for DisplayList {
    fn clone(&self) -> Self {
        let mut list = Self::new();
        list.append(self, None);
        list.chunks.clone_from(&self.chunks);
        list.ink = self.ink;
        list
    }
}

fn is_blend(blend: BlendMode) -> bool {
    blend != BlendMode::default()
}

impl DisplayList {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.items.clear();
        self.transforms.clear();
        self.paint_transforms.clear();
        self.radii.clear();
        self.paths.clear();
        self.runs.clear();
        self.glyphs.clear();
        self.coords.clear();
        self.open.clear();
        self.blends_at_root = false;
        self.chunks.clear();
        self.ink = NO_INK;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn open_layers(&self) -> usize {
        self.open.len()
    }

    #[must_use]
    pub fn layers(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item, Item::PushClip { .. } | Item::PushLayer { .. }))
            .count()
    }

    #[must_use]
    pub fn glyph_runs(&self) -> usize {
        self.runs.len()
    }

    #[cfg(test)]
    pub(crate) fn image_draws(&self) -> usize {
        self.items
            .iter()
            .filter(|item| match item {
                Item::Fill(draw) | Item::Stroke(draw, _) => {
                    matches!(draw.paint, Paint::Image(_) | Paint::Texture(_))
                }
                _ => false,
            })
            .count()
    }

    pub(crate) fn items(&self) -> &[Item] {
        &self.items
    }

    pub(crate) fn runs(&self) -> &[GlyphRun] {
        &self.runs
    }

    pub(crate) fn glyphs_of(&self, run: &GlyphRun) -> &[Glyph] {
        &self.glyphs[run.glyphs.start as usize..run.glyphs.end as usize]
    }

    pub(crate) fn coords_of(&self, run: &GlyphRun) -> &[NormalizedCoord] {
        &self.coords[run.coords.start as usize..run.coords.end as usize]
    }

    pub(crate) fn transform(&self, index: u32) -> Affine {
        self.transforms[index as usize]
    }

    pub(crate) fn paint_transform(&self, index: u32) -> Option<Affine> {
        (index != NO_TRANSFORM).then(|| self.paint_transforms[index as usize])
    }

    pub(crate) fn rounded(&self, rect: Rect, radii: u32) -> RoundedRect {
        RoundedRect::from_rect(rect, self.radii[radii as usize])
    }

    fn transform_index(&mut self, transform: Affine) -> u32 {
        if self.transforms.last() != Some(&transform) {
            self.transforms.push(transform);
        }
        index(self.transforms.len() - 1)
    }

    fn paint_transform_index(&mut self, transform: Option<Affine>) -> u32 {
        let Some(transform) = transform else {
            return NO_TRANSFORM;
        };
        if self.paint_transforms.last() != Some(&transform) {
            self.paint_transforms.push(transform);
        }
        index(self.paint_transforms.len() - 1)
    }

    pub(crate) fn path(&self, start: u32, end: u32) -> &[PathEl] {
        &self.paths[start as usize..end as usize]
    }

    pub(crate) fn blends_at_root(&self) -> bool {
        self.blends_at_root
    }

    fn shape(&mut self, shape: &impl crate::kurbo::Shape, tolerance: f64) -> Option<Shape> {
        if let Some(rect) = shape.as_rect() {
            return Some(Shape::Rect(rect));
        }
        if let Some(rounded) = shape.as_rounded_rect() {
            if rounded.radii().as_single_radius() == Some(0.0) {
                return Some(Shape::Rect(rounded.rect()));
            }
            self.radii.push(rounded.radii());
            return Some(Shape::RoundedRect {
                rect: rounded.rect(),
                radii: index(self.radii.len() - 1),
            });
        }
        let start = self.paths.len();
        self.paths.extend(shape.path_elements(tolerance));
        let end = self.paths.len();
        if start == end {
            return None;
        }
        Some(Shape::Path {
            start: index(start),
            end: index(end),
        })
    }

    fn record(&mut self, item: Item) {
        if !self.chunks.is_empty() {
            self.chunks.clear();
            self.ink = NO_INK;
        }
        self.items.push(item);
    }

    pub fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl crate::kurbo::Shape,
    ) {
        self.fill_paint(
            style,
            transform,
            brush.into().into(),
            brush_transform,
            shape,
        );
    }

    pub(crate) fn fill_paint(
        &mut self,
        style: Fill,
        transform: Affine,
        paint: Paint,
        brush_transform: Option<Affine>,
        shape: &impl crate::kurbo::Shape,
    ) {
        let Some(shape) = self.shape(shape, FILL_TOLERANCE) else {
            return;
        };
        let draw = Draw {
            rule: style,
            transform: self.transform_index(transform),
            paint_transform: self.paint_transform_index(brush_transform),
            shape,
            paint,
        };
        self.record(Item::Fill(draw));
    }

    pub fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl crate::kurbo::Shape,
    ) {
        if style.width == 0.0 {
            return;
        }
        let Some(shape) = self.shape(shape, STROKE_TOLERANCE) else {
            return;
        };
        let draw = Draw {
            rule: Fill::NonZero,
            transform: self.transform_index(transform),
            paint_transform: self.paint_transform_index(brush_transform),
            shape,
            paint: brush.into().into(),
        };
        self.record(Item::Stroke(draw, Arc::new(style.clone())));
    }

    pub(crate) fn stroke_paint(
        &mut self,
        style: &Stroke,
        transform: Affine,
        paint: Paint,
        brush_transform: Option<Affine>,
        shape: &impl crate::kurbo::Shape,
    ) {
        if style.width == 0.0 {
            return;
        }
        let Some(shape) = self.shape(shape, STROKE_TOLERANCE) else {
            return;
        };
        let draw = Draw {
            rule: Fill::NonZero,
            transform: self.transform_index(transform),
            paint_transform: self.paint_transform_index(brush_transform),
            shape,
            paint,
        };
        self.record(Item::Stroke(draw, Arc::new(style.clone())));
    }

    #[must_use]
    pub(crate) fn recorded_bytes(&self) -> usize {
        use std::mem::size_of;
        self.items.len() * size_of::<Item>()
            + self.transforms.len() * size_of::<Affine>()
            + self.paint_transforms.len() * size_of::<Affine>()
            + self.radii.len() * size_of::<RoundedRectRadii>()
            + self.paths.len() * size_of::<PathEl>()
            + self.runs.len() * size_of::<GlyphRun>()
            + self.glyphs.len() * size_of::<Glyph>()
            + self.coords.len() * size_of::<NormalizedCoord>()
    }

    fn region(&mut self, rule: Fill, transform: Affine, clip: &impl crate::kurbo::Shape) -> Region {
        let shape = self
            .shape(clip, FILL_TOLERANCE)
            .unwrap_or(Shape::Rect(Rect::ZERO));
        Region {
            rule,
            transform: self.transform_index(transform),
            shape,
        }
    }

    pub fn push_layer(
        &mut self,
        clip_style: Fill,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl crate::kurbo::Shape,
    ) {
        let region = self.region(clip_style, transform, clip);
        self.open_layer(Item::PushLayer {
            region,
            blend: blend.into(),
            alpha: alpha.clamp(0.0, 1.0),
        });
    }

    pub fn push_clip_layer(
        &mut self,
        clip_style: Fill,
        transform: Affine,
        clip: &impl crate::kurbo::Shape,
    ) {
        let region = self.region(clip_style, transform, clip);
        self.open_layer(Item::PushClip {
            region,
            isolated: false,
            isolable: true,
        });
    }

    pub(crate) fn push_backdrop_clip(
        &mut self,
        clip_style: Fill,
        transform: Affine,
        clip: &impl crate::kurbo::Shape,
    ) {
        let region = self.region(clip_style, transform, clip);
        self.open_layer(Item::PushClip {
            region,
            isolated: false,
            isolable: false,
        });
    }

    pub(crate) fn push_drop_shadow(&mut self, dx: f64, dy: f64, std_dev: f64, color: Color) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the rasterizer takes filter parameters as f32"
        )]
        self.open_layer(Item::PushShadow(DropShadow {
            dx: dx as f32,
            dy: dy as f32,
            std_dev: std_dev as f32,
            color,
        }));
    }

    fn open_layer(&mut self, item: Item) {
        if let Item::PushLayer { blend, .. } = &item
            && is_blend(*blend)
        {
            self.isolate_enclosing_clip();
        }
        self.open.push(index(self.items.len()));
        self.record(item);
    }

    fn isolate_enclosing_clip(&mut self) {
        if self.open.is_empty() {
            self.blends_at_root = true;
        }
        for &open in self.open.iter().rev() {
            match &mut self.items[open as usize] {
                Item::PushClip {
                    isolated, isolable, ..
                } => {
                    *isolated = *isolable;
                    break;
                }
                Item::PushLayer { .. } | Item::PushShadow(_) => break,
                _ => {}
            }
        }
    }

    pub fn pop_layer(&mut self) {
        if self.open.pop().is_some() {
            self.record(Item::Pop);
        }
    }

    pub fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: Rect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        let transform = self.transform_index(transform);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the rasterizer takes the radius and deviation as f32"
        )]
        self.record(Item::BlurredRoundedRect(BlurredRoundedRect {
            transform,
            rect,
            color: brush,
            radius: radius as f32,
            std_dev: std_dev as f32,
        }));
    }

    pub fn draw_image<'b>(&mut self, image: impl Into<ImageBrushRef<'b>>, transform: Affine) {
        let brush = image.into();
        let rect = Rect::new(
            0.0,
            0.0,
            f64::from(brush.image.width),
            f64::from(brush.image.height),
        );
        self.fill(Fill::NonZero, transform, brush, None, &rect);
    }

    pub fn draw_glyphs(&mut self, font: &FontData) -> DrawGlyphs<'_> {
        DrawGlyphs {
            run: GlyphRun {
                font: font.clone(),
                font_size: 16.0,
                transform: Affine::IDENTITY,
                glyph_transform: None,
                paint: Paint::Solid(Color::BLACK),
                paint_transform: None,
                style: GlyphStyle::Fill,
                hint: false,
                embolden: Embolden::default(),
                glyphs: 0..0,
                coords: 0..0,
            },
            coords_start: self.coords.len(),
            brush_alpha: 1.0,
            list: self,
        }
    }

    pub fn append(&mut self, other: &Self, transform: Option<Affine>) {
        self.append_chunks(other, transform, None);
    }

    pub(crate) fn append_visible(&mut self, other: &Self, transform: Affine, visible: Rect) {
        if other.chunks.is_empty() {
            self.append(other, Some(transform));
            return;
        }
        debug_assert_eq!(
            other.chunks.last().map(|chunk| chunk.end as usize),
            Some(other.items.len()),
            "a sealed list records nothing after its seal",
        );
        if !may_reach(other.ink, transform, visible) {
            return;
        }
        self.append_chunks(other, Some(transform), Some(visible));
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per item kind, each remapping its arena indices"
    )]
    fn append_chunks(&mut self, other: &Self, transform: Option<Affine>, visible: Option<Rect>) {
        let outer = |inner: Affine| transform.map_or(inner, |outer| outer * inner);
        let transforms = index(self.transforms.len());
        let paint_transforms = index(self.paint_transforms.len());
        let radii = index(self.radii.len());
        let paths = index(self.paths.len());
        let glyphs = index(self.glyphs.len());
        let coords = index(self.coords.len());
        self.transforms
            .extend(other.transforms.iter().map(|&inner| outer(inner)));
        self.paint_transforms
            .extend_from_slice(&other.paint_transforms);
        self.radii.extend_from_slice(&other.radii);
        self.paths.extend_from_slice(&other.paths);
        self.glyphs.extend_from_slice(&other.glyphs);
        self.coords.extend_from_slice(&other.coords);
        let shape = |shape: Shape| match shape {
            Shape::Path { start, end } => Shape::Path {
                start: start + paths,
                end: end + paths,
            },
            Shape::RoundedRect { rect, radii: at } => Shape::RoundedRect {
                rect,
                radii: at + radii,
            },
            other @ Shape::Rect(_) => other,
        };
        let region = |region: Region| Region {
            transform: region.transform + transforms,
            shape: shape(region.shape),
            ..region
        };
        let draw = |draw: &Draw| Draw {
            transform: draw.transform + transforms,
            paint_transform: if draw.paint_transform == NO_TRANSFORM {
                NO_TRANSFORM
            } else {
                draw.paint_transform + paint_transforms
            },
            shape: shape(draw.shape),
            ..draw.clone()
        };
        let whole = [Chunk {
            end: index(other.items.len()),
            bounds: ANYWHERE,
        }];
        let (chunks, cull) = match (visible, transform) {
            (Some(visible), Some(transform)) if !other.chunks.is_empty() => {
                (other.chunks.as_slice(), Some((transform, visible)))
            }
            _ => (whole.as_slice(), None),
        };
        self.items.reserve(other.items.len());
        let mut start = 0;
        for chunk in chunks {
            let items = &other.items[start..chunk.end as usize];
            start = chunk.end as usize;
            if cull.is_some_and(|(transform, visible)| !may_reach(chunk.bounds, transform, visible))
            {
                continue;
            }
            for item in items {
                match item {
                    Item::Fill(fill) => self.record(Item::Fill(draw(fill))),
                    Item::Stroke(stroke, style) => {
                        self.record(Item::Stroke(draw(stroke), Arc::clone(style)));
                    }
                    Item::PushClip {
                        region: clip,
                        isolable,
                        ..
                    } => self.open_layer(Item::PushClip {
                        region: region(*clip),
                        isolated: false,
                        isolable: *isolable,
                    }),
                    Item::PushShadow(shadow) => self.open_layer(Item::PushShadow(*shadow)),
                    Item::PushLayer {
                        region: layer,
                        blend,
                        alpha,
                    } => self.open_layer(Item::PushLayer {
                        region: region(*layer),
                        blend: *blend,
                        alpha: *alpha,
                    }),
                    Item::Pop => self.pop_layer(),
                    Item::Glyphs(run) => {
                        let run = &other.runs[*run as usize];
                        self.record(Item::Glyphs(index(self.runs.len())));
                        self.runs.push(GlyphRun {
                            transform: outer(run.transform),
                            glyphs: run.glyphs.start + glyphs..run.glyphs.end + glyphs,
                            coords: run.coords.start + coords..run.coords.end + coords,
                            ..run.clone()
                        });
                    }
                    Item::BlurredRoundedRect(blur) => {
                        self.record(Item::BlurredRoundedRect(BlurredRoundedRect {
                            transform: blur.transform + transforms,
                            ..*blur
                        }));
                    }
                }
            }
        }
    }

    pub(crate) fn seal_bounds(&mut self) {
        let mut chunks = std::mem::take(&mut self.chunks);
        chunks.clear();
        let mut ink = NO_INK;
        let mut clips: SmallVec<[Rect; 8]> = SmallVec::new();
        let mut chunk = NO_INK;
        let mut gathered = 0;
        let mut face: Option<(u64, u32, Rect)> = None;
        for (at, item) in self.items.iter().enumerate() {
            let clip = clips.last().copied();
            match item {
                Item::Fill(draw) => {
                    let bounds = self.shape_bounds(draw.shape);
                    let ink = self.transform(draw.transform).transform_rect_bbox(bounds);
                    grow(&mut chunk, cut(ink, clip));
                }
                Item::Stroke(draw, stroke) => {
                    let reach = stroke_reach(stroke);
                    let bounds = self.shape_bounds(draw.shape).inflate(reach, reach);
                    let ink = self.transform(draw.transform).transform_rect_bbox(bounds);
                    grow(&mut chunk, cut(ink, clip));
                }
                Item::PushClip { region, .. } => {
                    let bounds = self
                        .transform(region.transform)
                        .transform_rect_bbox(self.shape_bounds(region.shape));
                    clips.push(cut(bounds, clip));
                }
                Item::PushLayer { region, blend, .. } => {
                    let bounds = cut(
                        self.transform(region.transform)
                            .transform_rect_bbox(self.shape_bounds(region.shape)),
                        clip,
                    );
                    if clears_under_empty_source(*blend) {
                        grow(&mut chunk, bounds);
                    }
                    clips.push(bounds);
                }
                Item::PushShadow(_) => {
                    grow(&mut chunk, ANYWHERE);
                    clips.push(clip.unwrap_or(ANYWHERE));
                }
                Item::Pop => {
                    clips.pop();
                }
                Item::Glyphs(run) => {
                    let run = &self.runs[*run as usize];
                    let id = run.font.data.id();
                    let em = match face {
                        Some((face_id, face_index, em))
                            if face_id == id && face_index == run.font.index =>
                        {
                            em
                        }
                        _ => {
                            let em = glyph_ink_em(run.font.data.data(), run.font.index);
                            face = Some((id, run.font.index, em));
                            em
                        }
                    };
                    grow(&mut chunk, cut(self.run_bounds(run, em), clip));
                }
                Item::BlurredRoundedRect(blur) => {
                    let reach = 3.0 * f64::from(blur.std_dev.abs()) + 1.0;
                    let ink = self
                        .transform(blur.transform)
                        .transform_rect_bbox(blur.rect.abs().inflate(reach, reach));
                    grow(&mut chunk, cut(ink, clip));
                }
            }
            gathered += 1;
            if clips.is_empty() && gathered >= CHUNK_ITEMS {
                close_chunk(&mut chunks, &mut ink, at + 1, chunk);
                chunk = NO_INK;
                gathered = 0;
            }
        }
        if gathered > 0 {
            close_chunk(&mut chunks, &mut ink, self.items.len(), chunk);
        }
        self.chunks = chunks;
        self.ink = ink;
    }

    #[cfg(test)]
    pub(crate) fn ink_bounds(&self) -> Option<Rect> {
        (!self.chunks.is_empty()).then_some(self.ink)
    }

    #[cfg(test)]
    pub(crate) fn chunks(&self) -> usize {
        self.chunks.len()
    }

    fn shape_bounds(&self, shape: Shape) -> Rect {
        match shape {
            Shape::Rect(rect) | Shape::RoundedRect { rect, .. } => rect.abs(),
            Shape::Path { start, end } => {
                let mut bounds = NO_INK;
                let mut add = |point: crate::kurbo::Point| {
                    bounds = Rect::new(
                        bounds.x0.min(point.x),
                        bounds.y0.min(point.y),
                        bounds.x1.max(point.x),
                        bounds.y1.max(point.y),
                    );
                };
                for element in self.path(start, end) {
                    match *element {
                        PathEl::MoveTo(p) | PathEl::LineTo(p) => add(p),
                        PathEl::QuadTo(a, b) => {
                            add(a);
                            add(b);
                        }
                        PathEl::CurveTo(a, b, c) => {
                            add(a);
                            add(b);
                            add(c);
                        }
                        PathEl::ClosePath => {}
                    }
                }
                bounds
            }
        }
    }

    fn run_bounds(&self, run: &GlyphRun, em: Rect) -> Rect {
        let glyphs = self.glyphs_of(run);
        let Some(first) = glyphs.first() else {
            return NO_INK;
        };
        let origins = glyphs.iter().fold(
            Rect::new(
                f64::from(first.x),
                f64::from(first.y),
                f64::from(first.x),
                f64::from(first.y),
            ),
            |origins, glyph| {
                let (x, y) = (f64::from(glyph.x), f64::from(glyph.y));
                Rect::new(
                    origins.x0.min(x),
                    origins.y0.min(y),
                    origins.x1.max(x),
                    origins.y1.max(y),
                )
            },
        );
        let size = f64::from(run.font_size.abs());
        let bold = run.embolden.x.abs().max(run.embolden.y.abs());
        let stroke = match &run.style {
            GlyphStyle::Fill => 0.0,
            GlyphStyle::Stroke(stroke) => stroke_reach(stroke),
        };
        let widen = bold + stroke + 1.0;
        let ink =
            Rect::new(em.x0 * size, em.y0 * size, em.x1 * size, em.y1 * size).inflate(widen, widen);
        let ink = run
            .glyph_transform
            .map_or(ink, |glyph| glyph.transform_rect_bbox(ink));
        run.transform.transform_rect_bbox(Rect::new(
            origins.x0 + ink.x0,
            origins.y0 + ink.y0,
            origins.x1 + ink.x1,
            origins.y1 + ink.y1,
        ))
    }

    #[cfg(test)]
    pub(crate) fn compare(&self, other: &Self) -> Result<(), String> {
        if self.items.len() != other.items.len() {
            return Err(format!(
                "{} items vs {}",
                self.items.len(),
                other.items.len()
            ));
        }
        for (position, (left, right)) in self.items.iter().zip(&other.items).enumerate() {
            if !self.same_item(left, other, right) {
                return Err(format!("item {position}: {left:?} vs {right:?}"));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn same_shape(&self, left: Shape, other: &Self, right: Shape) -> bool {
        match (left, right) {
            (Shape::Rect(left), Shape::Rect(right)) => left == right,
            (
                Shape::RoundedRect { rect, radii },
                Shape::RoundedRect {
                    rect: other_rect,
                    radii: other_radii,
                },
            ) => self.rounded(rect, radii) == other.rounded(other_rect, other_radii),
            (Shape::Path { start, end }, Shape::Path { start: s, end: e }) => {
                self.path(start, end) == other.path(s, e)
            }
            _ => false,
        }
    }

    #[cfg(test)]
    fn same_draw(&self, left: &Draw, other: &Self, right: &Draw) -> bool {
        left.rule == right.rule
            && self.transform(left.transform) == other.transform(right.transform)
            && left.paint == right.paint
            && self.paint_transform(left.paint_transform)
                == other.paint_transform(right.paint_transform)
            && self.same_shape(left.shape, other, right.shape)
    }

    #[cfg(test)]
    fn same_region(&self, left: &Region, other: &Self, right: &Region) -> bool {
        left.rule == right.rule
            && self.transform(left.transform) == other.transform(right.transform)
            && self.same_shape(left.shape, other, right.shape)
    }

    #[cfg(test)]
    #[expect(
        clippy::float_cmp,
        reason = "the same drawing records the same floats; the comparison is exact by design"
    )]
    fn same_item(&self, left: &Item, other: &Self, right: &Item) -> bool {
        match (left, right) {
            (Item::Fill(left), Item::Fill(right)) => self.same_draw(left, other, right),
            (Item::Stroke(left, style), Item::Stroke(right, other_style)) => {
                style == other_style && self.same_draw(left, other, right)
            }
            (
                Item::PushClip {
                    region,
                    isolated,
                    isolable,
                },
                Item::PushClip {
                    region: other_region,
                    isolated: other_isolated,
                    isolable: other_isolable,
                },
            ) => {
                isolated == other_isolated
                    && isolable == other_isolable
                    && self.same_region(region, other, other_region)
            }
            (Item::PushShadow(left), Item::PushShadow(right)) => left == right,
            (
                Item::PushLayer {
                    region,
                    blend,
                    alpha,
                },
                Item::PushLayer {
                    region: other_region,
                    blend: other_blend,
                    alpha: other_alpha,
                },
            ) => {
                blend == other_blend
                    && alpha == other_alpha
                    && self.same_region(region, other, other_region)
            }
            (Item::Pop, Item::Pop) => true,
            (Item::Glyphs(left), Item::Glyphs(right)) => {
                let (left, right) = (&self.runs[*left as usize], &other.runs[*right as usize]);
                left.font == right.font
                    && left.font_size == right.font_size
                    && left.transform == right.transform
                    && left.glyph_transform == right.glyph_transform
                    && left.paint == right.paint
                    && left.paint_transform == right.paint_transform
                    && left.style == right.style
                    && left.hint == right.hint
                    && left.embolden == right.embolden
                    && self.glyphs_of(left) == other.glyphs_of(right)
                    && self.coords_of(left) == other.coords_of(right)
            }
            (Item::BlurredRoundedRect(left), Item::BlurredRoundedRect(right)) => {
                self.transform(left.transform) == other.transform(right.transform)
                    && left.rect == right.rect
                    && left.color == right.color
                    && left.radius == right.radius
                    && left.std_dev == right.std_dev
            }
            _ => false,
        }
    }
}

fn index(length: usize) -> u32 {
    u32::try_from(length).expect("a display list holds fewer than 2^32 of anything")
}

#[must_use = "a glyph run is recorded by `draw`"]
#[derive(Debug)]
pub struct DrawGlyphs<'a> {
    list: &'a mut DisplayList,
    run: GlyphRun,
    coords_start: usize,
    brush_alpha: f32,
}

impl DrawGlyphs<'_> {
    pub fn font_size(mut self, size: f32) -> Self {
        self.run.font_size = size;
        self
    }

    pub fn transform(mut self, transform: Affine) -> Self {
        self.run.transform = transform;
        self
    }

    pub fn glyph_transform(mut self, transform: Option<Affine>) -> Self {
        self.run.glyph_transform = transform;
        self
    }

    pub fn font_embolden(mut self, embolden: Embolden) -> Self {
        self.run.embolden = embolden;
        self
    }

    pub fn hint(mut self, hint: bool) -> Self {
        self.run.hint = hint;
        self
    }

    pub fn normalized_coords(self, coords: &[NormalizedCoord]) -> Self {
        self.list.coords.truncate(self.coords_start);
        self.list.coords.extend_from_slice(coords);
        self
    }

    pub fn brush<'b>(mut self, brush: impl Into<BrushRef<'b>>) -> Self {
        self.run.paint = brush.into().into();
        self
    }

    pub(crate) fn paint(mut self, paint: Paint) -> Self {
        self.run.paint = paint;
        self
    }

    pub fn brush_alpha(mut self, alpha: f32) -> Self {
        self.brush_alpha = alpha;
        self
    }

    pub fn brush_transform(mut self, transform: Option<Affine>) -> Self {
        self.run.paint_transform = transform;
        self
    }

    pub fn draw<'s>(self, style: impl Into<StyleRef<'s>>, glyphs: impl Iterator<Item = Glyph>) {
        let Self {
            list,
            mut run,
            coords_start,
            brush_alpha,
        } = self;
        let glyphs_start = list.glyphs.len();
        list.glyphs.extend(glyphs);
        if list.glyphs.len() == glyphs_start {
            list.coords.truncate(coords_start);
            return;
        }
        run.style = match style.into() {
            StyleRef::Fill(_) => GlyphStyle::Fill,
            StyleRef::Stroke(stroke) => GlyphStyle::Stroke(Arc::new(stroke.clone())),
        };
        run.paint = run.paint.multiply_alpha(brush_alpha);
        run.glyphs = index(glyphs_start)..index(list.glyphs.len());
        run.coords = index(coords_start)..index(list.coords.len());
        list.record(Item::Glyphs(index(list.runs.len())));
        list.runs.push(run);
    }
}

fn u16_at(font: &[u8], at: usize) -> Option<u16> {
    font.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn head_table(font: &[u8], index: u32) -> Option<usize> {
    let u32_at = |at: usize| {
        font.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let face = if font.get(..4) == Some(b"ttcf") {
        u32_at(12 + 4 * index as usize)? as usize
    } else {
        0
    };
    let tables = usize::from(u16_at(font, face + 4)?);
    (0..tables).find_map(|table| {
        let record = face + 12 + 16 * table;
        (font.get(record..record + 4)? == b"head")
            .then(|| u32_at(record + 8).map(|offset| offset as usize))
            .flatten()
    })
}

fn glyph_ink_em(font: &[u8], index: u32) -> Rect {
    let head = head_table(font, index);
    let upem = head
        .and_then(|head| u16_at(font, head + 18))
        .filter(|&upem| upem > 0)
        .map_or(1000.0, f64::from);
    let signed = |at: usize| u16_at(font, at).map(|bits| f64::from(bits.cast_signed()));
    let face = head.and_then(|head| {
        Some(Rect::new(
            signed(head + 36)? / upem,
            -signed(head + 42)? / upem,
            signed(head + 40)? / upem,
            -signed(head + 38)? / upem,
        ))
    });
    let floor = face.map_or(GLYPH_FLOOR_EM, |face| face.abs().union(GLYPH_FLOOR_EM));
    floor.inflate(GLYPH_MARGIN_EM, GLYPH_MARGIN_EM)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::kurbo::{Line, RoundedRect};
    use crate::peniko::{Compose, Mix};

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 8.0, 8.0)
    }

    #[test]
    fn an_item_is_at_most_96_bytes() {
        assert!(
            std::mem::size_of::<Item>() <= 96,
            "{} bytes",
            std::mem::size_of::<Item>()
        );
    }

    #[test]
    fn rectangles_stay_rectangles() {
        let mut list = DisplayList::new();
        list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        let rounded = RoundedRect::from_rect(rect(), 2.0);
        list.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &rounded,
        );
        list.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Line::new((0.0, 0.0), (4.0, 0.0)),
        );
        let shapes: Vec<_> = list
            .items()
            .iter()
            .map(|item| match item {
                Item::Fill(draw) | Item::Stroke(draw, _) => draw.shape,
                _ => unreachable!("only draws were recorded"),
            })
            .collect();
        assert!(matches!(shapes[0], Shape::Rect(r) if r == rect()));
        assert!(
            matches!(shapes[1], Shape::RoundedRect { rect, radii } if list.rounded(rect, radii) == rounded)
        );
        assert!(matches!(shapes[2], Shape::Path { start: 0, end: 2 }));
    }

    fn isolated_clips(list: &DisplayList) -> Vec<bool> {
        list.items()
            .iter()
            .filter_map(|item| match item {
                Item::PushClip { isolated, .. } => Some(*isolated),
                _ => None,
            })
            .collect()
    }

    fn blend(list: &mut DisplayList) {
        list.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Saturation, Compose::SrcAtop),
            1.0,
            Affine::IDENTITY,
            &rect(),
        );
        list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        list.pop_layer();
    }

    #[test]
    fn the_clip_directly_around_a_blend_is_isolated() {
        let mut list = DisplayList::new();
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        blend(&mut list);
        list.pop_layer();
        list.pop_layer();
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        list.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            0.5,
            Affine::IDENTITY,
            &rect(),
        );
        blend(&mut list);
        list.pop_layer();
        list.pop_layer();
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        list.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            0.5,
            Affine::IDENTITY,
            &rect(),
        );
        list.pop_layer();
        list.pop_layer();
        assert_eq!(isolated_clips(&list), [false, true, false, false]);
        assert!(!list.blends_at_root());
        assert_eq!(list.open_layers(), 0);
    }

    #[test]
    fn an_appended_blend_isolates_the_hosts_clip_directly_around_it() {
        let mut bare = DisplayList::new();
        blend(&mut bare);
        assert!(bare.blends_at_root());
        let mut clipped = DisplayList::new();
        clipped.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        blend(&mut clipped);
        clipped.pop_layer();
        assert_eq!(isolated_clips(&clipped), [true]);
        assert!(!clipped.blends_at_root());

        let mut host = DisplayList::new();
        host.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        host.append(&bare, Some(Affine::translate((2.0, 0.0))));
        host.pop_layer();
        host.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        host.append(&clipped, Some(Affine::translate((2.0, 0.0))));
        host.pop_layer();
        assert_eq!(
            isolated_clips(&host),
            [true, false, true],
            "the host's clip around the bare blend; the fragment's own clip, not the host's, \
             around the clipped one",
        );
        assert!(!host.blends_at_root());
        assert_eq!(host.open_layers(), 0);
    }

    fn near_then_far() -> DisplayList {
        let mut list = DisplayList::new();
        for _ in 0..CHUNK_ITEMS {
            list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        }
        list.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Rect::new(500.0, 0.0, 508.0, 8.0),
        );
        list
    }

    const TARGET: Rect = Rect::new(0.0, 0.0, 100.0, 100.0);

    #[test]
    fn recording_after_a_seal_unseals_the_list() {
        let mut list = DisplayList::new();
        for _ in 0..CHUNK_ITEMS {
            list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        }
        list.seal_bounds();
        assert_eq!(list.chunks(), 1);
        list.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Rect::new(500.0, 0.0, 508.0, 8.0),
        );
        assert_eq!(list.ink_bounds(), None, "unsealed");
        let mut whole = DisplayList::new();
        whole.append_visible(&list, Affine::IDENTITY, TARGET);
        assert_eq!(whole.len(), CHUNK_ITEMS + 1, "appended whole");

        list.seal_bounds();
        assert_eq!(list.chunks(), 2);
        let mut culled = DisplayList::new();
        culled.append_visible(&list, Affine::IDENTITY, TARGET);
        assert_eq!(culled.len(), CHUNK_ITEMS, "the far fill is left out again");
    }

    #[test]
    fn a_clone_of_a_sealed_list_keeps_its_seal() {
        let mut list = near_then_far();
        list.seal_bounds();
        let clone = list.clone();
        assert_eq!(clone.chunks(), list.chunks());
        assert_eq!(clone.ink_bounds(), list.ink_bounds());
        let mut culled = DisplayList::new();
        culled.append_visible(&clone, Affine::IDENTITY, TARGET);
        assert_eq!(culled.len(), CHUNK_ITEMS, "the far fill is left out");
        assert_eq!(
            near_then_far().clone().ink_bounds(),
            None,
            "unsealed stays unsealed"
        );
    }

    #[test]
    fn an_append_is_the_same_drawing_recorded_in_place() {
        let font = FontData::new(crate::peniko::Blob::new(Arc::new([0_u8; 4])), 0);
        let record = |list: &mut DisplayList, outer: Affine| {
            list.stroke(
                &Stroke::new(1.0),
                outer,
                Color::BLACK,
                None,
                &Line::new((0.0, 0.0), (4.0, 0.0)),
            );
            list.draw_glyphs(&font)
                .transform(outer * Affine::translate((1.0, 2.0)))
                .normalized_coords(&[3, 4])
                .draw(
                    Fill::NonZero,
                    [Glyph {
                        id: 7,
                        x: 1.0,
                        y: 2.0,
                    }]
                    .into_iter(),
                );
        };
        let outer = Affine::translate((5.0, 6.0));
        let mut fragment = DisplayList::new();
        record(&mut fragment, Affine::IDENTITY);
        let mut appended = DisplayList::new();
        appended.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &rect());
        appended.append(&fragment, Some(outer));
        let mut expected = DisplayList::new();
        expected.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &rect());
        record(&mut expected, outer);
        assert_eq!(appended.compare(&expected), Ok(()));
    }

    #[test]
    fn a_gradient_of_one_stop_is_its_colour() {
        let mut list = DisplayList::new();
        let one =
            Gradient::new_linear((0.0, 0.0), (1.0, 0.0)).with_stops([Color::WHITE].as_slice());
        list.fill(Fill::NonZero, Affine::IDENTITY, &one, None, &rect());
        let none = Gradient::new_linear((0.0, 0.0), (1.0, 0.0));
        list.fill(Fill::NonZero, Affine::IDENTITY, &none, None, &rect());
        let paints: Vec<_> = list
            .items()
            .iter()
            .map(|item| match item {
                Item::Fill(draw) => draw.paint.clone(),
                _ => unreachable!("only fills were recorded"),
            })
            .collect();
        assert_eq!(
            paints,
            [Paint::Solid(Color::WHITE), Paint::Solid(Color::TRANSPARENT)]
        );
    }

    fn sealed(mut list: DisplayList) -> Rect {
        list.seal_bounds();
        list.ink_bounds()
            .expect("a sealed list with items has a bound")
    }

    fn holds(outer: Rect, inner: Rect) -> bool {
        outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1
    }

    #[test]
    fn a_strokes_bound_covers_its_width_and_its_joins() {
        let mut list = DisplayList::new();
        let stroke = Stroke::new(4.0)
            .with_join(Join::Miter)
            .with_miter_limit(4.0);
        list.stroke(
            &stroke,
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Line::new((0.0, 0.0), (10.0, 0.0)),
        );
        let bounds = sealed(list);
        assert!(
            holds(bounds, Rect::new(-8.0, -8.0, 18.0, 8.0)),
            "{bounds:?}"
        );
    }

    #[test]
    fn a_shadows_bound_covers_its_blur() {
        let mut list = DisplayList::new();
        list.draw_blurred_rounded_rect(
            Affine::translate((100.0, 0.0)),
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Color::BLACK,
            2.0,
            4.0,
        );
        let bounds = sealed(list);
        assert!(
            holds(bounds, Rect::new(90.0, -10.0, 120.0, 20.0)),
            "{bounds:?}"
        );
    }

    #[test]
    fn a_transformed_draw_is_bounded_by_its_mapped_box_cut_to_its_clip() {
        let mut list = DisplayList::new();
        let rotated = Affine::translate((50.0, 50.0)) * Affine::rotate(std::f64::consts::FRAC_PI_4);
        list.fill(
            Fill::NonZero,
            rotated,
            Color::BLACK,
            None,
            &Rect::new(-10.0, -10.0, 10.0, 10.0),
        );
        let bounds = sealed(list);
        let half = 10.0 * std::f64::consts::SQRT_2;
        let mapped = Rect::new(50.0 - half, 50.0 - half, 50.0 + half, 50.0 + half);
        assert!(holds(bounds, mapped), "{bounds:?}");
        assert!(holds(mapped.inflate(1e-9, 1e-9), bounds), "{bounds:?}");

        let mut clipped = DisplayList::new();
        clipped.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 50.0, 50.0),
        );
        clipped.fill(
            Fill::NonZero,
            rotated,
            Color::BLACK,
            None,
            &Rect::new(-10.0, -10.0, 10.0, 10.0),
        );
        clipped.pop_layer();
        let bounds = sealed(clipped);
        let expected = Rect::new(50.0 - half, 50.0 - half, 50.0, 50.0);
        assert!(
            holds(bounds, expected.inflate(-1e-9, -1e-9))
                && holds(expected.inflate(1e-9, 1e-9), bounds),
            "{bounds:?}"
        );
    }

    #[test]
    fn a_layer_that_clears_under_it_is_bounded_by_its_region() {
        let region = Rect::new(0.0, 0.0, 100.0, 100.0);
        let content = Rect::new(10.0, 10.0, 20.0, 20.0);
        let layer = |blend: BlendMode| {
            let mut list = DisplayList::new();
            list.push_layer(Fill::NonZero, blend, 1.0, Affine::IDENTITY, &region);
            list.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::BLACK,
                None,
                &content,
            );
            list.pop_layer();
            sealed(list)
        };
        assert_eq!(layer(BlendMode::new(Mix::Normal, Compose::Copy)), region);
        assert_eq!(
            layer(BlendMode::new(Mix::Multiply, Compose::SrcOver)),
            content
        );
    }

    #[test]
    fn a_glyph_runs_bound_covers_an_em_around_its_glyphs() {
        let font = FontData::new(crate::peniko::Blob::new(Arc::new([0_u8; 4])), 0);
        let mut list = DisplayList::new();
        list.draw_glyphs(&font)
            .font_size(10.0)
            .transform(Affine::translate((0.0, 200.0)))
            .draw(
                Fill::NonZero,
                [
                    Glyph {
                        id: 1,
                        x: 0.0,
                        y: 0.0,
                    },
                    Glyph {
                        id: 2,
                        x: 100.0,
                        y: 0.0,
                    },
                ]
                .into_iter(),
            );
        let bounds = sealed(list);
        assert!(
            holds(bounds, Rect::new(-10.0, 190.0, 110.0, 210.0)),
            "{bounds:?}"
        );
    }

    #[test]
    fn a_faces_box_grows_its_runs_bound() {
        let mut font = vec![0_u8; 28 + 54];
        font[4..6].copy_from_slice(&1_u16.to_be_bytes());
        font[12..16].copy_from_slice(b"head");
        font[20..24].copy_from_slice(&28_u32.to_be_bytes());
        font[28 + 18..28 + 20].copy_from_slice(&1000_u16.to_be_bytes());
        font[28 + 40..28 + 42].copy_from_slice(&5000_i16.to_be_bytes());
        font[28 + 42..28 + 44].copy_from_slice(&4000_i16.to_be_bytes());
        let em = glyph_ink_em(&font, 0);
        assert!(holds(em, Rect::new(0.0, -4.0, 5.0, 0.0)), "{em:?}");
    }

    #[test]
    fn chunks_are_cut_only_where_no_layer_is_open() {
        let mut list = DisplayList::new();
        for _ in 0..5 {
            list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        }
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        for _ in 0..6 {
            list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        }
        list.pop_layer();
        list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        list.seal_bounds();
        assert_eq!(list.chunks(), 3);
        let ends: Vec<u32> = list.chunks.iter().map(|chunk| chunk.end).collect();
        assert_eq!(ends, [4, 13, 14]);
    }

    #[test]
    fn a_culled_append_keeps_the_chunks_that_reach_and_balances() {
        let font = FontData::new(crate::peniko::Blob::new(Arc::new([0_u8; 4])), 0);
        let glyph = |list: &mut DisplayList, x: f32| {
            list.draw_glyphs(&font)
                .draw(Fill::NonZero, [Glyph { id: 1, x, y: 0.0 }].into_iter());
        };
        let mut fragment = DisplayList::new();
        fragment.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            0.5,
            Affine::IDENTITY,
            &Rect::new(500.0, 0.0, 600.0, 10.0),
        );
        fragment.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Rect::new(500.0, 0.0, 510.0, 10.0),
        );
        fragment.pop_layer();
        glyph(&mut fragment, 520.0);
        for _ in 0..3 {
            fragment.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &rect());
        }
        glyph(&mut fragment, 4.0);
        fragment.seal_bounds();
        assert_eq!(fragment.chunks(), 2);

        let mut culled = DisplayList::new();
        culled.append_visible(
            &fragment,
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        assert_eq!(culled.len(), 4, "the second chunk alone");
        assert_eq!(culled.layers(), 0);
        assert_eq!(culled.open_layers(), 0);
        assert_eq!(culled.glyph_runs(), 1);
        assert!(matches!(culled.items()[3], Item::Glyphs(0)));
        assert!(
            (culled.runs()[0].glyphs.start..culled.runs()[0].glyphs.end).contains(&1),
            "the run keeps its own glyph"
        );

        let mut moved = DisplayList::new();
        moved.append_visible(
            &fragment,
            Affine::translate((-480.0, 0.0)),
            Rect::new(-500.0, 0.0, 100.0, 100.0),
        );
        let mut whole = DisplayList::new();
        whole.append(&fragment, Some(Affine::translate((-480.0, 0.0))));
        assert_eq!(moved.compare(&whole), Ok(()));

        let mut away = DisplayList::new();
        away.append_visible(
            &fragment,
            Affine::translate((0.0, 1000.0)),
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        assert!(away.is_empty());
    }

    #[test]
    fn a_bound_that_is_not_finite_always_reaches() {
        let visible = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(may_reach(ANYWHERE, Affine::IDENTITY, visible));
        assert!(!may_reach(NO_INK, Affine::IDENTITY, visible));
        assert!(may_reach(
            Rect::new(100.0, 100.0, 110.0, 110.0),
            Affine::translate((f64::NAN, 0.0)),
            visible
        ));
        assert!(!may_reach(
            Rect::new(100.0, 100.0, 110.0, 110.0),
            Affine::IDENTITY,
            visible
        ));
        assert!(may_reach(
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Affine::IDENTITY,
            visible
        ));
    }

    #[test]
    fn a_collection_face_reads_its_own_head_table() {
        let mut font = vec![0_u8; 28 + 54];
        font[4..6].copy_from_slice(&1_u16.to_be_bytes());
        font[12..16].copy_from_slice(b"head");
        font[20..24].copy_from_slice(&28_u32.to_be_bytes());
        font[28 + 18..28 + 20].copy_from_slice(&2048_u16.to_be_bytes());
        font[28 + 40..28 + 42].copy_from_slice(&5000_i16.to_be_bytes());
        let plain = glyph_ink_em(&font, 0);
        assert!(
            holds(plain, Rect::new(0.0, 0.0, 5000.0 / 2048.0, 0.0)),
            "{plain:?}"
        );
        let mut collection = vec![0_u8; 20];
        collection[..4].copy_from_slice(b"ttcf");
        collection[16..20].copy_from_slice(&20_u32.to_be_bytes());
        let mut face = font.clone();
        face[20..24].copy_from_slice(&(20_u32 + 28).to_be_bytes());
        collection.extend_from_slice(&face);
        assert_eq!(glyph_ink_em(&collection, 1), plain);
        assert_ne!(
            glyph_ink_em(&[0; 12], 0),
            plain,
            "no head table: the floor box"
        );
    }

    #[test]
    fn synthetic_bold_is_stored_in_font_size_units() {
        let font = FontData::new(crate::peniko::Blob::new(Arc::new([0_u8; 4])), 0);
        let glyph = [Glyph {
            id: 1,
            x: 0.0,
            y: 0.0,
        }];
        let mut list = DisplayList::new();
        for style in [
            StyleRef::Fill(Fill::NonZero),
            StyleRef::Stroke(&Stroke::new(1.0)),
        ] {
            list.draw_glyphs(&font)
                .font_size(20.0)
                .font_embolden(Embolden::uniform(0.5))
                .draw(style, glyph.into_iter());
        }
        let [fill, stroke] = list.runs() else {
            unreachable!("two runs were recorded")
        };
        assert_eq!(fill.embolden, Embolden::uniform(0.5));
        assert_eq!(stroke.embolden, Embolden::uniform(0.5));
    }
}
