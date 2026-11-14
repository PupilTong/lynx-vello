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

/// One positioned glyph of a run.
pub use glifo::Glyph;
use smallvec::SmallVec;

use crate::kurbo::{Affine, Join, PathEl, Rect, RoundedRect, RoundedRectRadii, Stroke};
use crate::peniko::{
    BlendMode, Blob, Brush, BrushRef, Color, Compose, Fill, FontData, Gradient, ImageBrushRef,
    ImageData, ImageSampler, StyleRef, WeakBlob,
};
use crate::render::image::ImageDescriptor;

/// A variation-space coordinate of a variable font, as `F2Dot14` bits.
pub type NormalizedCoord = i16;

/// The tolerance a curved fill or clip shape is flattened to a path at.
pub(crate) const FILL_TOLERANCE: f64 = 0.1;

/// The tolerance a curved stroked shape is flattened to a path at. The
/// stroke is expanded from the flattened path, so the flattening error moves
/// both of its edges; a tenth of a fill's tolerance keeps that far below a
/// device pixel.
pub(crate) const STROKE_TOLERANCE: f64 = 0.01;

/// The geometry one item draws or clips with, in the item's own space.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Shape {
    Rect(Rect),
    /// A rounded rectangle: its rectangle, and its radii in the list's radii
    /// arena.
    RoundedRect {
        rect: Rect,
        radii: u32,
    },
    /// A path: a range of the list's path arena.
    Path {
        start: u32,
        end: u32,
    },
}

/// The index an item stores for "no paint transform".
const NO_TRANSFORM: u32 = u32::MAX;

/// The paint a fill, stroke or glyph run applies.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    Solid(Color),
    /// Shared, so a list that copies the draw copies a reference.
    Gradient(Arc<Gradient>),
    /// A bitmap and how it is sampled.
    Image(Arc<ImagePaint>),
    /// A texture the rasterizer holds, and how it is sampled.
    Texture(Arc<TexturePaint>),
}

/// A texture paint: the texture, and how it is sampled.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TexturePaint {
    pub(crate) texture: Texture,
    pub(crate) sampler: ImageSampler,
}

impl Paint {
    /// The paint drawing `texture` through `sampler`.
    pub(crate) fn texture(texture: Texture, sampler: ImageSampler) -> Self {
        Self::Texture(Arc::new(TexturePaint { texture, sampler }))
    }
}

/// A texture the rasterizer holds — a `filter: blur()` or `backdrop-filter`
/// bake, a vector image's bake — named by the process-unique id it is bound
/// under (`render::raster::Raster::bind_texture`). A draw refers to it by
/// that id alone: there are no pixels here and nothing to look up.
///
/// The picture's size is what a draw's brush scale divides by. The texture
/// may be longer by one texel on an axis whose last column or row a bake
/// repeated (`render::vector_textures`), which a padded draw samples and a
/// repeating one does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texture {
    id: u64,
    width: u32,
    height: u32,
    stored_width: u32,
    stored_height: u32,
}

impl Texture {
    /// A fresh id for a `width` × `height` picture held in a texture of
    /// `texture_width` × `texture_height`.
    pub(crate) fn new(width: u32, height: u32, texture_width: u32, texture_height: u32) -> Self {
        Self {
            id: next_texture_id(),
            width,
            height,
            stored_width: texture_width,
            stored_height: texture_height,
        }
    }

    /// The id the texture is bound under, unique in the process.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The picture's size.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The texture's own size.
    pub(crate) fn texture_size(&self) -> (u32, u32) {
        (self.stored_width, self.stored_height)
    }
}

/// A texture id no other texture in the process has had: the rasterizer
/// draws a texture by this id directly, its bakes' and its own bitmaps'
/// alike, so they share one counter.
pub(crate) fn next_texture_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A bitmap paint: the bitmap, and how it is sampled.
///
/// Equal paints are the same bitmap through the same sampler, whoever
/// holds its pixels.
#[derive(Clone, Debug)]
pub(crate) struct ImagePaint {
    pub(crate) image: ImageRef,
    pub(crate) sampler: ImageSampler,
}

impl PartialEq for ImagePaint {
    fn eq(&self, other: &Self) -> bool {
        self.image.descriptor == other.image.descriptor && self.sampler == other.sampler
    }
}

/// A bitmap a list draws: what it is, and where its pixels are.
///
/// The rasterizer keys its upload of a bitmap on the descriptor alone, so a
/// draw whose upload is resident never reads the pixels; it needs them only
/// to upload. A composed list lives until the next compose — on a static
/// screen, indefinitely — so a bitmap the host's store owns is lent to it
/// rather than held, and the store stays free to drop it once the
/// rasterizer holds its upload.
#[derive(Clone, Debug)]
pub struct ImageRef {
    pub descriptor: ImageDescriptor,
    pub pixels: ImagePixels,
}

/// Where an [`ImageRef`]'s pixels are.
#[derive(Clone, Debug)]
pub enum ImagePixels {
    /// Held by the draw: a picture nothing else owns — a bitmap a list
    /// was handed directly, or one a caller with no rasterizer to lease from
    /// resolved for a scene it keeps.
    Held(Blob<u8>),
    /// Lent by whoever owns them: drawable while they are alive.
    Lent(WeakBlob<u8>),
    /// Nowhere on the CPU: only an upload the rasterizer holds has them.
    Uploaded,
}

impl ImageRef {
    /// `image`, held.
    #[must_use]
    pub fn held(image: &ImageData) -> Self {
        Self {
            descriptor: ImageDescriptor::of(image),
            pixels: ImagePixels::Held(image.data.clone()),
        }
    }

    /// `image`, lent: its owner keeps the pixels alive.
    #[must_use]
    pub fn lent(image: &ImageData) -> Self {
        Self {
            descriptor: ImageDescriptor::of(image),
            pixels: ImagePixels::Lent(image.data.downgrade()),
        }
    }

    /// The bitmap `descriptor` names, whose pixels only an upload holds.
    #[must_use]
    pub fn uploaded(descriptor: ImageDescriptor) -> Self {
        Self {
            descriptor,
            pixels: ImagePixels::Uploaded,
        }
    }

    /// The pixels, when they are still alive somewhere.
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
    /// This paint with its alpha multiplied by `alpha`.
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

/// A gradient brush as a paint: one with no stops paints nothing and one
/// with a single stop is that stop's colour, which is what a gradient of
/// one colour draws (SVG 2 §14.3.3 says so of a paint server, and the CSS
/// gradient functions need two stops to parse).
fn gradient_paint(gradient: &Gradient) -> Paint {
    match gradient.stops.as_slice() {
        [] => Paint::Solid(Color::TRANSPARENT),
        [only] => Paint::Solid(only.color.to_alpha_color()),
        _ => Paint::Gradient(Arc::new(gradient.clone())),
    }
}

/// A fill or a stroke. Its transforms are indices into the list's arenas
/// ([`DisplayList::transform`], [`DisplayList::paint_transform`]), which
/// keeps an item small and lets an append map each distinct transform once.
#[derive(Clone, Debug)]
pub(crate) struct Draw {
    pub(crate) rule: Fill,
    /// The shape's space to the list's.
    pub(crate) transform: u32,
    /// The paint's space to the shape's.
    pub(crate) paint_transform: u32,
    pub(crate) shape: Shape,
    pub(crate) paint: Paint,
}

/// A clip or a group layer: its shape and fill rule.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Region {
    pub(crate) rule: Fill,
    /// The shape's space to the list's.
    pub(crate) transform: u32,
    pub(crate) shape: Shape,
}

/// A Gaussian-blurred rounded rectangle (`box-shadow`): the area
/// `std_dev × 2.5` around `rect` is painted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlurredRoundedRect {
    /// The rectangle's space to the list's.
    pub(crate) transform: u32,
    pub(crate) rect: Rect,
    pub(crate) color: Color,
    pub(crate) radius: f32,
    pub(crate) std_dev: f32,
}

/// One recorded drawing operation.
#[derive(Clone, Debug)]
pub(crate) enum Item {
    Fill(Draw),
    Stroke(Draw, Arc<Stroke>),
    /// A plain clip: everything up to the matching [`Item::Pop`] is cut to
    /// the region.
    ///
    /// `isolated` is set when a [`Item::PushLayer`] that blends by anything
    /// but source-over opens inside it with no other clip or group layer
    /// between: such a clip is composited as a layer of its own, so the
    /// blend reads only what was painted inside the clip and the clip
    /// applies once, when it composites. A clip further out composites
    /// plain source-over, and a group layer between isolates the blend by
    /// itself, so neither needs a layer of its own: every other clip is
    /// intersected into what it holds (`render::sparse`).
    PushClip {
        region: Region,
        isolated: bool,
    },
    /// A group layer: its content composited by `blend` at `alpha`, cut to
    /// the region.
    PushLayer {
        region: Region,
        blend: BlendMode,
        alpha: f32,
    },
    Pop,
    /// [`DisplayList::runs`]`[index]`.
    Glyphs(u32),
    BlurredRoundedRect(BlurredRoundedRect),
}

/// How a glyph run's outlines are drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GlyphStyle {
    Fill,
    Stroke(Arc<Stroke>),
}

/// Synthetic bold: how far a run's outlines are widened, with the controls
/// of the widening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Embolden {
    /// Per-axis widening.
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
    /// Widening by `amount` font-size units on both axes, with the default
    /// controls.
    #[must_use]
    pub fn uniform(amount: f64) -> Self {
        Self {
            x: amount,
            y: amount,
            ..Self::default()
        }
    }
}

/// One glyph run.
#[derive(Clone, Debug)]
pub(crate) struct GlyphRun {
    pub(crate) font: FontData,
    pub(crate) font_size: f32,
    /// The run's space to the list's.
    pub(crate) transform: Affine,
    /// Applied to each glyph's outline at its origin, in the run's y-down
    /// space (a synthetic oblique's shear).
    pub(crate) glyph_transform: Option<Affine>,
    pub(crate) paint: Paint,
    /// The paint's space to the run's; `None` is the identity.
    pub(crate) paint_transform: Option<Affine>,
    pub(crate) style: GlyphStyle,
    pub(crate) hint: bool,
    /// Synthetic bold, in font-size units: glifo widens each glyph by it at
    /// the run's font size and scales the widening with the run's
    /// transform, whatever size it outlines the glyph at.
    pub(crate) embolden: Embolden,
    pub(crate) glyphs: Range<u32>,
    pub(crate) coords: Range<u32>,
}

/// See the [module documentation](self).
#[derive(Default)]
pub struct DisplayList {
    items: Vec<Item>,
    /// Item transforms, a run of items under one transform sharing an entry.
    transforms: Vec<Affine>,
    /// Paint transforms, relative to their item's transform.
    paint_transforms: Vec<Affine>,
    radii: Vec<RoundedRectRadii>,
    paths: Vec<PathEl>,
    runs: Vec<GlyphRun>,
    glyphs: Vec<Glyph>,
    coords: Vec<NormalizedCoord>,
    /// The indices of the open clips and layers, innermost last.
    open: Vec<u32>,
    /// Whether a layer that blends by anything but source-over opens with
    /// no clip or layer around it, so that it blends with what is under the
    /// whole list.
    blends_at_root: bool,
    /// The chunks [`Self::seal_bounds`] cut the items into, in order; empty
    /// for a list not sealed since it last changed.
    chunks: Vec<Chunk>,
    /// The union of the chunks' bounds: [`NO_INK`] when they put none.
    ink: Rect,
}

/// A run of items that opens and closes its own clips and layers, and a
/// bound on where its items can put ink, in the list's space.
#[derive(Clone, Copy, Debug)]
struct Chunk {
    /// The index one past the chunk's last item.
    end: u32,
    /// [`NO_INK`] when the chunk puts none; not finite when no finite bound
    /// was established, which never culls.
    bounds: Rect,
}

/// How many items a chunk gathers before it is cut at the next point where
/// no clip or layer is open. Each chunk costs one rectangle test per frame;
/// a few items each keeps that small beside what one item costs to
/// rasterize, while a chunk straddling the target's edge drags in little
/// that is off the target.
const CHUNK_ITEMS: usize = 4;

/// The bound of a run of items that puts no ink: empty under every union,
/// and outside every rectangle.
const NO_INK: Rect = Rect::new(
    f64::INFINITY,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NEG_INFINITY,
);

/// No finite bound: whatever holds it is never culled.
const ANYWHERE: Rect = Rect::new(
    f64::NEG_INFINITY,
    f64::NEG_INFINITY,
    f64::INFINITY,
    f64::INFINITY,
);

/// `acc` grown to cover `ink`. Ink with a coordinate that is not a number
/// has no bound, so it makes `acc` [`ANYWHERE`]; ink of no area adds
/// nothing, since it covers no pixel.
fn grow(acc: &mut Rect, ink: Rect) {
    if ink.x0.is_nan() || ink.y0.is_nan() || ink.x1.is_nan() || ink.y1.is_nan() {
        *acc = ANYWHERE;
    } else if ink.x1 > ink.x0 && ink.y1 > ink.y0 {
        *acc = acc.union(ink);
    }
}

/// `ink` cut to `clip`, the bound of the clips open around it; `None` is
/// no clip. Either side not being a number leaves the other: a clip of no
/// bound cuts nothing, and ink of no bound fills the clip.
fn cut(ink: Rect, clip: Option<Rect>) -> Rect {
    match clip {
        Some(clip) if ink.x0.is_nan() || ink.y0.is_nan() || ink.x1.is_nan() || ink.y1.is_nan() => {
            clip
        }
        Some(clip) => ink.intersect(clip),
        None => ink,
    }
}

/// Whether ink bounded by `bounds` in some space, mapped by `transform`,
/// can reach `visible`. Only a proof of the opposite answers `false`: a
/// bound or map that is not finite, or a comparison with something that is
/// not a number, answers `true`. Edges that touch count as reaching.
pub(crate) fn may_reach(bounds: Rect, transform: Affine, visible: Rect) -> bool {
    if bounds.x0 > bounds.x1 || bounds.y0 > bounds.y1 {
        // [`NO_INK`]: nothing to reach with.
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

/// Records a chunk ending before item `end` with `bounds`, and grows the
/// list's `ink` by it.
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

/// How far a stroke of `stroke` can put ink past its path's bounding box,
/// in the path's space: half the width, as far again as the miter limit
/// lets a miter join reach, and a square cap's half diagonal.
fn stroke_reach(stroke: &Stroke) -> f64 {
    let half = stroke.width.abs() / 2.0;
    let join = if stroke.join == Join::Miter {
        stroke.miter_limit.max(1.0)
    } else {
        1.0
    };
    half * join.max(std::f64::consts::SQRT_2)
}

/// Whether a layer composited by `blend` changes what is under it where
/// its content put no ink: a compose whose destination factor is not one
/// for a transparent source. Such a layer puts ink over its whole region.
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

/// How far, in ems, the outline of a glyph can reach from its origin
/// beyond the face's own bounding box (`head`): a variable instance's
/// outlines may leave the default instance's box, and a face's box may be
/// wrong. Glyph runs are a small share of what a chunk straddling the
/// target's edge drags in, so the margin is generous.
const GLYPH_MARGIN_EM: f64 = 1.0;

/// The box, in ems with y down, every glyph's ink is assumed to reach at
/// least, whatever the face's own box says.
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
    /// The same drawing, sealed the same: a clone of a sealed list keeps
    /// its chunks and its bound, since the copy has the same items at the
    /// same indices.
    fn clone(&self) -> Self {
        let mut list = Self::new();
        list.append(self, None);
        list.chunks.clone_from(&self.chunks);
        list.ink = self.ink;
        list
    }
}

/// Whether `blend` is anything but plain source-over.
fn is_blend(blend: BlendMode) -> bool {
    blend != BlendMode::default()
}

impl DisplayList {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties the list, keeping its capacity.
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

    /// Whether the list records nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many items the list records: draws, glyph runs, and every push
    /// and pop.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// How many clips and layers are open at the end of the list.
    #[must_use]
    pub fn open_layers(&self) -> usize {
        self.open.len()
    }

    /// How many clips and group layers the list pushes.
    #[must_use]
    pub fn layers(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item, Item::PushClip { .. } | Item::PushLayer { .. }))
            .count()
    }

    /// How many glyph runs the list records.
    #[must_use]
    pub fn glyph_runs(&self) -> usize {
        self.runs.len()
    }

    /// How many fills and strokes paint a bitmap or a texture.
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

    /// The glyphs of `run`.
    pub(crate) fn glyphs_of(&self, run: &GlyphRun) -> &[Glyph] {
        &self.glyphs[run.glyphs.start as usize..run.glyphs.end as usize]
    }

    /// The variation coordinates of `run`.
    pub(crate) fn coords_of(&self, run: &GlyphRun) -> &[NormalizedCoord] {
        &self.coords[run.coords.start as usize..run.coords.end as usize]
    }

    /// Item transform `index`.
    pub(crate) fn transform(&self, index: u32) -> Affine {
        self.transforms[index as usize]
    }

    /// Paint transform `index`, `None` for the identity.
    pub(crate) fn paint_transform(&self, index: u32) -> Option<Affine> {
        (index != NO_TRANSFORM).then(|| self.paint_transforms[index as usize])
    }

    /// The rounded rectangle of a [`Shape::RoundedRect`].
    pub(crate) fn rounded(&self, rect: Rect, radii: u32) -> RoundedRect {
        RoundedRect::from_rect(rect, self.radii[radii as usize])
    }

    /// The index of `transform` in the item transform arena, reusing the
    /// last entry when it is the same.
    fn transform_index(&mut self, transform: Affine) -> u32 {
        if self.transforms.last() != Some(&transform) {
            self.transforms.push(transform);
        }
        index(self.transforms.len() - 1)
    }

    /// The index of `transform` in the paint transform arena, reusing the
    /// last entry when it is the same.
    fn paint_transform_index(&mut self, transform: Option<Affine>) -> u32 {
        let Some(transform) = transform else {
            return NO_TRANSFORM;
        };
        if self.paint_transforms.last() != Some(&transform) {
            self.paint_transforms.push(transform);
        }
        index(self.paint_transforms.len() - 1)
    }

    /// The path elements of a [`Shape::Path`].
    pub(crate) fn path(&self, start: u32, end: u32) -> &[PathEl] {
        &self.paths[start as usize..end as usize]
    }

    /// Whether some layer blends by anything but source-over with what is
    /// under the whole list.
    pub(crate) fn blends_at_root(&self) -> bool {
        self.blends_at_root
    }

    /// Records `shape` as an item shape at `tolerance`; `None` for a shape
    /// with no segments.
    fn shape(&mut self, shape: &impl crate::kurbo::Shape, tolerance: f64) -> Option<Shape> {
        if let Some(rect) = shape.as_rect() {
            return Some(Shape::Rect(rect));
        }
        if let Some(rounded) = shape.as_rounded_rect() {
            // Square corners are a rectangle, and keep its fast paths.
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

    /// Appends `item`. Every item lands here, and any item recorded after
    /// [`Self::seal_bounds`] unseals the list: its chunks no longer cover
    /// every item, so they are dropped and the list is appended whole until
    /// it is sealed again, rather than appended short of its tail.
    fn record(&mut self, item: Item) {
        if !self.chunks.is_empty() {
            self.chunks.clear();
            self.ink = NO_INK;
        }
        self.items.push(item);
    }

    /// Fills `shape` under `transform` with `brush`, placed by
    /// `brush_transform` relative to the shape's space.
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

    /// [`Self::fill`] with a paint: what draws a texture.
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

    /// Strokes `shape` under `transform` with `brush`. A dash pattern on
    /// `style` is applied by the rasterizer.
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

    /// The region of a clip or layer over `clip`; an empty shape clips
    /// everything.
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

    /// Opens a group layer: what is drawn until the matching
    /// [`Self::pop_layer`] is composited by `blend` at `alpha`, cut to
    /// `clip` under `transform`.
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

    /// Opens a clip: what is drawn until the matching [`Self::pop_layer`]
    /// is cut to `clip` under `transform`.
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
        });
    }

    /// Records a push, keeping the open stack and the clips' isolation.
    fn open_layer(&mut self, item: Item) {
        if let Item::PushLayer { blend, .. } = &item
            && is_blend(*blend)
        {
            self.isolate_enclosing_clip();
        }
        self.open.push(index(self.items.len()));
        self.record(item);
    }

    /// Marks the clip a blend layer is opening directly inside isolated:
    /// the innermost open clip, unless a group layer lies between it and
    /// the blend, which isolates the blend by itself. Clips further out
    /// composite plain source-over whatever happens inside, so they are
    /// left as they are ([`Item::PushClip`]).
    fn isolate_enclosing_clip(&mut self) {
        if self.open.is_empty() {
            self.blends_at_root = true;
        }
        for &open in self.open.iter().rev() {
            match &mut self.items[open as usize] {
                Item::PushClip { isolated, .. } => {
                    *isolated = true;
                    break;
                }
                Item::PushLayer { .. } => break,
                _ => {}
            }
        }
    }

    /// Closes the innermost open clip or layer.
    pub fn pop_layer(&mut self) {
        if self.open.pop().is_some() {
            self.record(Item::Pop);
        }
    }

    /// Draws a rounded rectangle blurred by a Gaussian of `std_dev`.
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

    /// Draws `image` at its natural size under `transform`.
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

    /// A builder for one glyph run of `font`.
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

    /// Appends `other`, every item's transform left-multiplied by
    /// `transform`.
    pub fn append(&mut self, other: &Self, transform: Option<Affine>) {
        self.append_chunks(other, transform, None);
    }

    /// Appends the chunks of `other` whose ink, mapped by `transform`, can
    /// reach `visible` in this list's space, every item's transform
    /// left-multiplied by `transform`. A list not sealed since it last
    /// changed is appended whole.
    ///
    /// What is left out is whole chunks, each of which closes every clip
    /// and layer it opens, so the layer stack balances as a whole append's
    /// does; and a chunk's bound covers everything its items can change on
    /// the target, a layer that clears what is under it included, so what
    /// is left out leaves no pixel of `visible` different.
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

    /// [`Self::append`], leaving out the chunks of `other` that cannot reach
    /// `visible` when it is set and `other` is sealed.
    fn append_chunks(&mut self, other: &Self, transform: Option<Affine>, visible: Option<Rect>) {
        let outer = |inner: Affine| transform.map_or(inner, |outer| outer * inner);
        let transforms = index(self.transforms.len());
        let paint_transforms = index(self.paint_transforms.len());
        let radii = index(self.radii.len());
        let paths = index(self.paths.len());
        let glyphs = index(self.glyphs.len());
        let coords = index(self.coords.len());
        // Each distinct item transform is mapped once; paint transforms are
        // relative to their item's and copy as they are.
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
        // One pseudo-chunk of every item when nothing is culled.
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
                    // Isolation is derived again as the items land, so a
                    // blend `other` opens at its root isolates the clip this
                    // list has open directly around the append.
                    Item::PushClip { region: clip, .. } => self.open_layer(Item::PushClip {
                        region: region(*clip),
                        isolated: false,
                    }),
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
                    // A run is copied as its item lands: runs are recorded
                    // in the order of the items naming them, one item each,
                    // so the copies keep that order.
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

    /// Cuts the items into chunks and bounds each one's ink (see the
    /// [module documentation](self)). A sealed list is a finished one:
    /// recording anything more unseals it ([`Self::record`]), and it is
    /// appended whole until it is sealed again.
    pub(crate) fn seal_bounds(&mut self) {
        let mut chunks = std::mem::take(&mut self.chunks);
        chunks.clear();
        let mut ink = NO_INK;
        // The bound of the clips and layers open at each item, innermost
        // last, each already cut to the ones around it.
        let mut clips: SmallVec<[Rect; 8]> = SmallVec::new();
        let mut chunk = NO_INK;
        let mut gathered = 0;
        // The last face a run was bounded with: runs of one face follow
        // each other, and its `head` table is read once for them.
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
                    // The rasterizer paints 2.5σ around the rectangle.
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

    /// The bound of a sealed list's ink in its own space, `None` when the
    /// list is not sealed: what [`Self::append_visible`] tests first.
    #[cfg(test)]
    pub(crate) fn ink_bounds(&self) -> Option<Rect> {
        (!self.chunks.is_empty()).then_some(self.ink)
    }

    /// How many chunks [`Self::seal_bounds`] cut the list into.
    #[cfg(test)]
    pub(crate) fn chunks(&self) -> usize {
        self.chunks.len()
    }

    /// The bounding box of `shape` in its own space.
    fn shape_bounds(&self, shape: Shape) -> Rect {
        match shape {
            Shape::Rect(rect) | Shape::RoundedRect { rect, .. } => rect.abs(),
            // A curve stays inside the hull of its control points, so their
            // box bounds the path.
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

    /// A bound on the ink of `run` in the list's space, given its face's
    /// glyph box `em` ([`glyph_ink_em`]).
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
        // Synthetic bold widens each side by its amount, in font-size units.
        let bold = run.embolden.x.abs().max(run.embolden.y.abs());
        let stroke = match &run.style {
            GlyphStyle::Fill => 0.0,
            GlyphStyle::Stroke(stroke) => stroke_reach(stroke),
        };
        // A pixel more for hinting, which moves outlines by less than one.
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
    /// Describes the first difference between this list and `other`, or
    /// `Ok` when they record the same drawing: the same items in the same
    /// order, with equal shapes, paints, transforms and glyphs. Floats
    /// compare with `==`.
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
                Item::PushClip { region, isolated },
                Item::PushClip {
                    region: other_region,
                    isolated: other_isolated,
                },
            ) => isolated == other_isolated && self.same_region(region, other, other_region),
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

/// An arena index or item count as the `u32` the list stores.
fn index(length: usize) -> u32 {
    u32::try_from(length).expect("a display list holds fewer than 2^32 of anything")
}

/// The builder [`DisplayList::draw_glyphs`] answers. Unset, a run is 16 px,
/// unhinted, untransformed and solid black, with no synthetic bold.
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

    /// The run's space to the list's.
    pub fn transform(mut self, transform: Affine) -> Self {
        self.run.transform = transform;
        self
    }

    /// Applied to each glyph's outline at its origin, in the run's y-down
    /// space.
    pub fn glyph_transform(mut self, transform: Option<Affine>) -> Self {
        self.run.glyph_transform = transform;
        self
    }

    /// Synthetic bold, in font-size units.
    pub fn font_embolden(mut self, embolden: Embolden) -> Self {
        self.run.embolden = embolden;
        self
    }

    pub fn hint(mut self, hint: bool) -> Self {
        self.run.hint = hint;
        self
    }

    /// The variation coordinates of a variable font's instance.
    pub fn normalized_coords(self, coords: &[NormalizedCoord]) -> Self {
        self.list.coords.truncate(self.coords_start);
        self.list.coords.extend_from_slice(coords);
        self
    }

    pub fn brush<'b>(mut self, brush: impl Into<BrushRef<'b>>) -> Self {
        self.run.paint = brush.into().into();
        self
    }

    /// A further alpha the brush is multiplied by.
    pub fn brush_alpha(mut self, alpha: f32) -> Self {
        self.brush_alpha = alpha;
        self
    }

    /// The paint's space to the run's.
    pub fn brush_transform(mut self, transform: Option<Affine>) -> Self {
        self.run.paint_transform = transform;
        self
    }

    /// Records the run of `glyphs`, filled or stroked as `style` says. A run
    /// with no glyph records nothing.
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

/// The big-endian `u16` at `at` in `font`.
fn u16_at(font: &[u8], at: usize) -> Option<u16> {
    font.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
}

/// The offset of face `index`'s `head` table in `font`, a font file or a
/// collection.
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

/// The box in ems, y down, that the outline of every glyph of face `index`
/// in `font` is assumed to stay inside, measured from the glyph's origin:
/// the face's `head` bounding box, which OpenType defines as the union of
/// every glyph's, grown to at least [`GLYPH_FLOOR_EM`] and then by
/// [`GLYPH_MARGIN_EM`] on every side.
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

    /// An item stays small: its transforms, radii and texture are indices
    /// or shared, so a list of thousands of draws stays close to the size
    /// of the drawing it records.
    #[test]
    fn an_item_is_at_most_96_bytes() {
        assert!(
            std::mem::size_of::<Item>() <= 96,
            "{} bytes",
            std::mem::size_of::<Item>()
        );
    }

    /// Shapes are kept as what they were recorded as: a rectangle and a
    /// rounded rectangle symbolically, anything else as a path.
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

    /// The `isolated` flag of every clip `list` pushes, in record order.
    fn isolated_clips(list: &DisplayList) -> Vec<bool> {
        list.items()
            .iter()
            .filter_map(|item| match item {
                Item::PushClip { isolated, .. } => Some(*isolated),
                _ => None,
            })
            .collect()
    }

    /// Opens a saturation blend layer holding one fill and closes it.
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

    /// The clip directly around a blend layer is isolated. A clip around
    /// that clip is not, nor a clip around an opacity layer that holds the
    /// blend, nor one around an opacity layer or a fill alone.
    #[test]
    fn the_clip_directly_around_a_blend_is_isolated() {
        let mut list = DisplayList::new();
        // Two clips, the blend in the inner one.
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        blend(&mut list);
        list.pop_layer();
        list.pop_layer();
        // A clip around an opacity layer around the blend: the layer
        // isolates the blend by itself.
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
        // A clip around an opacity layer alone.
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

    /// Appending re-derives isolation across the seam: a blend a fragment
    /// opens at its root isolates the clip its host has open around the
    /// append, and a fragment whose own clip holds the blend leaves the
    /// host's clip alone.
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

    /// `CHUNK_ITEMS` fills on the target, then one far off it.
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

    /// The 100 px target the seal tests append for.
    const TARGET: Rect = Rect::new(0.0, 0.0, 100.0, 100.0);

    /// Recording after a seal unseals the list, which is then appended
    /// whole — never short of the items recorded late — until it is sealed
    /// again.
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

    /// A clone of a sealed list is sealed the same, and culls the same.
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

    /// An append maps every transform and lands the arenas at their new
    /// offsets, so the copy compares equal to the same drawing recorded in
    /// place.
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

    /// A gradient of one stop is that colour, and one of none paints
    /// nothing.
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

    /// The sealed bound of `list`'s ink.
    fn sealed(mut list: DisplayList) -> Rect {
        list.seal_bounds();
        list.ink_bounds()
            .expect("a sealed list with items has a bound")
    }

    /// Whether `outer` holds all of `inner`.
    fn holds(outer: Rect, inner: Rect) -> bool {
        outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1
    }

    /// A stroke's bound covers half its width past the path on every side,
    /// and its miter joins as far as the miter limit lets them reach.
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
        // Half the width, as far again as a miter of limit 4 reaches.
        assert!(
            holds(bounds, Rect::new(-8.0, -8.0, 18.0, 8.0)),
            "{bounds:?}"
        );
    }

    /// A box shadow's bound covers the 2.5σ around its rectangle the
    /// rasterizer paints, under the shadow's transform.
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

    /// A transformed draw is bounded by its mapped box, and a clip around
    /// it cuts the bound.
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

    /// A layer that clears what is under it where its content is empty
    /// (here `copy`) puts ink over its whole region; one composited
    /// source-over puts only its content's.
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

    /// A glyph run is bounded by its glyphs' origins grown by an em box
    /// under the run's transform, so its ink inside an em of its origins
    /// is never outside the bound.
    #[test]
    fn a_glyph_runs_bound_covers_an_em_around_its_glyphs() {
        // No `head` table: the floor box alone.
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

    /// The face's own `head` box grows a run's bound past the floor box.
    #[test]
    fn a_faces_box_grows_its_runs_bound() {
        // `head` at 28: unitsPerEm 1000, a box 5 ems wide and 4 tall above
        // the baseline.
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

    /// Chunks are cut every few items, never inside an open clip or layer.
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
        // Four fills; one fill and the whole clip; the last fill.
        assert_eq!(list.chunks(), 3);
        let ends: Vec<u32> = list.chunks.iter().map(|chunk| chunk.end).collect();
        assert_eq!(ends, [4, 13, 14]);
    }

    /// A culled append leaves out the chunks that cannot reach the visible
    /// rectangle and keeps every other, with their glyph runs, and the
    /// layer stack it leaves is balanced.
    #[test]
    fn a_culled_append_keeps_the_chunks_that_reach_and_balances() {
        let font = FontData::new(crate::peniko::Blob::new(Arc::new([0_u8; 4])), 0);
        let glyph = |list: &mut DisplayList, x: f32| {
            list.draw_glyphs(&font)
                .draw(Fill::NonZero, [Glyph { id: 1, x, y: 0.0 }].into_iter());
        };
        let mut fragment = DisplayList::new();
        // Off the target, a layer and a run among them.
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
        // On it.
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

        // Moved onto the target, both chunks land: a whole append.
        let mut moved = DisplayList::new();
        moved.append_visible(
            &fragment,
            Affine::translate((-480.0, 0.0)),
            Rect::new(-500.0, 0.0, 100.0, 100.0),
        );
        let mut whole = DisplayList::new();
        whole.append(&fragment, Some(Affine::translate((-480.0, 0.0))));
        assert_eq!(moved.compare(&whole), Ok(()));

        // Nowhere near: nothing.
        let mut away = DisplayList::new();
        away.append_visible(
            &fragment,
            Affine::translate((0.0, 1000.0)),
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        assert!(away.is_empty());
    }

    /// Only a proof culls: a bound or map that is not finite reaches.
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
        // Touching edges reach.
        assert!(may_reach(
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Affine::IDENTITY,
            visible
        ));
    }

    /// A face's `head` table is found in a plain font and in a collection,
    /// and a font without one falls back to the floor box.
    #[test]
    fn a_collection_face_reads_its_own_head_table() {
        // An offset table of one record, `head` at 28: unitsPerEm 2048 and
        // a box 5000 units wide.
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
        // The same face as the second of a collection.
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

    /// Synthetic bold is stored as recorded, in font-size units, for a fill
    /// and a stroke alike: glifo converts it to the size it outlines at.
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
