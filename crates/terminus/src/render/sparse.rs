//! The rasterizer: a [`DisplayList`] replayed into a `vello_gpu::Scene` and
//! rendered by sparse strips on `vello_gpu`'s wgpu backend.
//! [`super::raster::Raster`] is its only caller.
//!
//! [`replay`] walks the list's items once per render and calls the scene's
//! drawing API for each, with the transform, fill rule, paint and paint
//! transform the item carries. This is the CPU half of a render: every path
//! is flattened and turned into strips here, on the target or not, so its
//! cost is the content's — which is why composition leaves out what cannot
//! reach the target before the list gets here
//! ([`crate::paint::compose::replay_ops`]).
//! A fill of a rectangle takes `vello_gpu`'s rectangle fast path, which
//! under an axis-aligned transform and clip records no strips at all. A
//! plain clip is intersected into the strips of what it holds with no layer
//! texture, a rectangle one by the same fast path, unless the list marked
//! it isolated because a blend opens inside it ([`Item::PushClip`]). A glyph
//! run is outlined by glifo through [`Glyphs`], with the outline cache the
//! backend keeps across renders.
//!
//! A texture paint (the `filter: blur()`, `backdrop-filter` and vector-image
//! bakes, bound by [`SparseBackend::bind`]) is drawn as an external texture
//! under its own id, sampled in place; a bitmap is uploaded once and
//! released once renders stop drawing it ([`Bitmaps`]).
//!
//! Every target on a device renders through the one `vello_gpu` renderer
//! and image atlas of its [`SharedGpu`] ([`SharedRaster`]); each target's
//! [`SparseBackend`] keeps its own scene, replay state and bound textures,
//! so the replay runs on the target's thread without the renderer's lock.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "target sizes and texture sides are small exact integers in u16 and u32 alike"
)]

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use glifo::{AtlasCacher, GlyphPrepCache, GlyphRenderError, GlyphRunBackend};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use vello_common::image_cache::ImageCache;
use vello_gpu::{
    AllocationStrategy, AtlasConfig, ClearSettings, Image, ImageId, ImageSource, LayersConfig,
    MemorySettings, PaintType, PixelMetadata, Pixmap, RectU16, RenderSettings, RenderSize,
    RenderTargetConfig, Renderer, Resources, Scene, SizeU16, TargetInit, TextureBindings,
    TextureId,
};

use super::gpu::SharedGpu;
use crate::clock::ClockInstant;
use crate::kurbo::{Affine, BezPath, Diagonal2, Point, Rect, Shape as _};
use crate::paint::display_list::{
    DisplayList, FILL_TOLERANCE, GlyphRun, GlyphStyle, ImagePaint, Item, Paint, STROKE_TOLERANCE,
    Shape, Texture, next_texture_id,
};
use crate::peniko::{
    Color, Extend, Fill, ImageAlphaType, ImageData, ImageFormat, ImageQuality, ImageSampler,
};

/// What one frame cost, and what it could not draw.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Stats {
    /// Replaying the list into the sparse-strip scene (path flattening and
    /// strip generation run here, on the CPU).
    pub(crate) encode: Duration,
    /// Waiting for another target's render to let the shared renderer go.
    pub(crate) wait: Duration,
    /// The queued bitmap uploads, `Renderer::render`, the releases and the
    /// submit: the span the shared renderer is held.
    pub(crate) render: Duration,
    pub(crate) draws: u32,
    pub(crate) glyph_runs: u32,
    pub(crate) layers: u32,
    /// Image draws with no readable pixels, and texture draws with no
    /// texture bound.
    pub(crate) images_skipped: u32,
}

/// One target's rasterizer: the scene its renders fill, the textures it
/// binds, and its pins on the bitmaps it draws, over the [`SharedRaster`]
/// of the [`SharedGpu`] it was built on.
pub(crate) struct SparseBackend {
    gpu: Arc<SharedGpu>,
    /// This target's name in the bitmaps' pins.
    target: u64,
    /// How many of this target's renders have drawn a bitmap.
    generation: u64,
    scene: Scene,
    /// What [`replay`] keeps across renders: glifo's outline cache and the
    /// scratch path.
    replay: Replay,
    /// The bound textures, by id.
    bound: FxHashMap<u64, wgpu::TextureView>,
    /// The bitmaps the current render draws from textures of their own,
    /// bound for it beside [`Self::bound`].
    textures: Vec<(TextureId, wgpu::TextureView)>,
    pub(crate) last: Stats,
}

impl std::fmt::Debug for SparseBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparseBackend")
            .field("target", &self.target)
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

/// The state [`replay`] keeps from one render to the next.
#[derive(Default)]
pub(crate) struct Replay {
    /// glifo's outline and hinting caches, maintained after every render.
    glyphs: GlyphPrepCache,
    /// The path a non-rectangular shape is rebuilt into for the scene.
    path: BezPath,
    /// Per open clip or layer, whether it was replayed as a non-isolated
    /// clip, which its pop closes with `pop_clip`.
    open: Vec<bool>,
}

impl Replay {
    /// Ends a render: lets glifo's outline cache age out what it no longer
    /// draws.
    pub(crate) fn maintain(&mut self) {
        self.glyphs.maintain();
    }
}

/// `shape` as a path in `list`'s arena, rebuilt into `path`.
fn shape_path<'p>(
    path: &'p mut BezPath,
    list: &DisplayList,
    shape: Shape,
    tolerance: f64,
) -> &'p BezPath {
    path.truncate(0);
    match shape {
        Shape::Rect(rect) => path.extend(rect.path_elements(tolerance)),
        Shape::RoundedRect { rect, radii } => {
            path.extend(list.rounded(rect, radii).path_elements(tolerance));
        }
        Shape::Path { start, end } => path.extend(list.path(start, end).iter().copied()),
    }
    path
}

/// Sets `paint` on `scene` with `transform` as its paint transform; `false`
/// when the paint draws nothing (an image with no pixels to draw).
fn set_paint(
    scene: &mut Scene,
    paint: &Paint,
    transform: Option<Affine>,
    pictures: &mut Pictures<'_>,
    stats: &mut Stats,
) -> bool {
    let paint: PaintType = match paint {
        Paint::Solid(color) => {
            scene.set_paint(*color);
            scene.reset_paint_transform();
            return true;
        }
        Paint::Gradient(gradient) => (**gradient).clone().into(),
        Paint::Image(image) => {
            let Some(image) = (pictures.bitmap)(image) else {
                stats.images_skipped += 1;
                return false;
            };
            image.into()
        }
        Paint::Texture(paint) => {
            // A texture a render cannot bind fails the whole render, so one
            // the bake that made it has since released draws nothing.
            if !(pictures.bound)(paint.texture.id()) {
                stats.images_skipped += 1;
                return false;
            }
            texture_image(paint.texture, paint.sampler).into()
        }
    };
    scene.set_paint(paint);
    scene.set_paint_transform(transform.unwrap_or(Affine::IDENTITY));
    true
}

/// How a replay resolves the pictures a list paints with.
pub(crate) struct Pictures<'a> {
    /// A bitmap paint to the image the scene draws; `None` draws nothing.
    pub(crate) bitmap: &'a mut dyn FnMut(&ImagePaint) -> Option<Image>,
    /// Whether a texture id is bound for the render.
    pub(crate) bound: &'a dyn Fn(u64) -> bool,
}

/// The image a texture paint draws: the texture under its own id, the
/// repeated edge a bake added included on a padded axis only.
fn texture_image(texture: Texture, sampler: ImageSampler) -> Image {
    let (width, height) = texture.size();
    let (texture_width, texture_height) = texture.texture_size();
    let region = |extend: Extend, size: u32, texture: u32| {
        let texture = u16::try_from(texture).unwrap_or(u16::MAX);
        let size = u16::try_from(size).map_or(texture, |size| size.min(texture));
        if extend == Extend::Pad && texture > size {
            size + 1
        } else {
            size
        }
    };
    Image {
        image: ImageSource::ExternalTexture {
            id: TextureId(texture.id()),
            source_region: RectU16::new(
                0,
                0,
                region(sampler.x_extend, width, texture_width),
                region(sampler.y_extend, height, texture_height),
            ),
            may_have_transparency: true,
        },
        sampler,
    }
}

/// Replays `list` into `scene`, which the caller has reset to its target
/// size, resolving its pictures through `pictures`.
pub(crate) fn replay(
    scene: &mut Scene,
    list: &DisplayList,
    kept: &mut Replay,
    pictures: &mut Pictures<'_>,
) -> Stats {
    let mut stats = Stats::default();
    let Replay { glyphs, path, open } = kept;
    open.clear();
    for item in list.items() {
        match item {
            Item::Fill(draw) => {
                if !set_paint(
                    scene,
                    &draw.paint,
                    list.paint_transform(draw.paint_transform),
                    pictures,
                    &mut stats,
                ) {
                    continue;
                }
                stats.draws += 1;
                scene.set_transform(list.transform(draw.transform));
                scene.set_fill_rule(draw.rule);
                match draw.shape {
                    // Under an axis-aligned transform and clip, a
                    // rectangle draws without strips.
                    Shape::Rect(rect) => scene.fill_rect(&rect),
                    shape => scene.fill_path(shape_path(path, list, shape, FILL_TOLERANCE)),
                }
            }
            Item::Stroke(draw, stroke) => {
                if !set_paint(
                    scene,
                    &draw.paint,
                    list.paint_transform(draw.paint_transform),
                    pictures,
                    &mut stats,
                ) {
                    continue;
                }
                stats.draws += 1;
                scene.set_transform(list.transform(draw.transform));
                // `vello_gpu` strokes through kurbo, which applies the dash
                // pattern the stroke carries.
                scene.set_stroke((**stroke).clone());
                scene.stroke_path(shape_path(path, list, draw.shape, STROKE_TOLERANCE));
            }
            Item::PushClip { region, isolated } => {
                stats.layers += 1;
                scene.set_transform(list.transform(region.transform));
                scene.set_fill_rule(region.rule);
                match (isolated, region.shape) {
                    // A non-isolated clip: no layer texture, its coverage
                    // intersected into the strips of what it holds. A layer
                    // pushed inside it composites cut to its own path within
                    // this clip.
                    (false, Shape::Rect(rect)) => scene.push_clip_rect(&rect),
                    (false, shape) => {
                        scene.push_clip_path(shape_path(path, list, shape, FILL_TOLERANCE));
                    }
                    (true, shape) => {
                        scene.push_clip_layer(shape_path(path, list, shape, FILL_TOLERANCE));
                    }
                }
                open.push(!isolated);
            }
            Item::PushLayer {
                region,
                blend,
                alpha,
            } => {
                stats.layers += 1;
                scene.set_transform(list.transform(region.transform));
                scene.set_fill_rule(region.rule);
                scene.push_layer(
                    Some(shape_path(path, list, region.shape, FILL_TOLERANCE)),
                    Some(*blend),
                    Some(*alpha),
                    None,
                    None,
                );
                open.push(false);
            }
            Item::Pop => match open.pop() {
                Some(true) => scene.pop_clip(),
                Some(false) => scene.pop_layer(),
                None => {}
            },
            Item::Glyphs(index) => {
                let run = &list.runs()[*index as usize];
                if set_paint(scene, &run.paint, run.paint_transform, pictures, &mut stats) {
                    stats.glyph_runs += 1;
                    replay_glyphs(scene, list, run, glyphs);
                }
            }
            Item::BlurredRoundedRect(blur) => {
                stats.draws += 1;
                scene.set_paint(blur.color);
                scene.reset_paint_transform();
                // Centred on its rect, the blur covers the 2.5σ-inflated
                // area around it.
                scene.set_transform(
                    list.transform(blur.transform)
                        .pre_translate(blur.rect.center().to_vec2()),
                );
                scene.fill_blurred_rounded_rect(
                    &Rect::from_center_size(Point::ORIGIN, blur.rect.size()),
                    blur.radius,
                    blur.std_dev,
                    false,
                );
            }
        }
    }
    stats
}

/// Draws `run` of `list` with the paint already set on `scene`, outlining
/// its glyphs through glifo with `cache`.
fn replay_glyphs(
    scene: &mut Scene,
    list: &DisplayList,
    run: &GlyphRun,
    cache: &mut GlyphPrepCache,
) {
    scene.set_transform(run.transform);
    scene.set_fill_rule(Fill::NonZero);
    if let GlyphStyle::Stroke(stroke) = &run.style {
        scene.set_stroke((**stroke).clone());
    }
    let full = run.transform * run.glyph_transform.unwrap_or(Affine::IDENTITY);
    let embolden = run.embolden.scaled(run.outline_scale(full));
    let mut builder = glifo::GlyphRunBuilder::new(
        run.font.clone(),
        run.transform,
        run.paint_transform.unwrap_or(Affine::IDENTITY),
        Glyphs { scene, cache },
    )
    .font_size(run.font_size)
    .hint(run.hint)
    .normalized_coords(list.coords_of(run))
    .font_embolden(glifo::FontEmbolden {
        amount: Diagonal2::new(embolden.x, embolden.y),
        join: embolden.join,
        miter_limit: embolden.miter_limit,
        tolerance: embolden.tolerance,
    });
    if let Some(transform) = run.glyph_transform {
        builder = builder.glyph_transform(transform);
    }
    let glyphs = list.glyphs_of(run).iter().copied();
    // A glyph the font cannot outline draws nothing.
    let _ = match run.style {
        GlyphStyle::Fill => builder.fill_glyphs(glyphs),
        GlyphStyle::Stroke(_) => builder.stroke_glyphs(glyphs),
    };
}

/// The CPU half of a render on its own, for benchmarks: a list replayed
/// into a sparse-strip scene, every path flattened and turned into strips,
/// with nothing submitted to a device.
///
/// A bitmap paint draws as a placeholder atlas image, whose fill is
/// stripped as any image fill is; what a real render adds is the upload,
/// once per bitmap.
pub struct StripScene {
    scene: Scene,
    replay: Replay,
}

impl std::fmt::Debug for StripScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripScene")
            .field("width", &self.scene.width())
            .field("height", &self.scene.height())
            .finish_non_exhaustive()
    }
}

impl StripScene {
    /// A scene for a `width` × `height` target.
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            scene: Scene::new(width, height),
            replay: Replay::default(),
        }
    }

    /// Replays `list` into the scene, emptied first, and answers how many
    /// draws and glyph runs it recorded.
    pub fn fill(&mut self, list: &DisplayList) -> u32 {
        let (width, height) = (self.scene.width(), self.scene.height());
        self.scene.reset_and_resize(width, height);
        let stats = replay(
            &mut self.scene,
            list,
            &mut self.replay,
            &mut Pictures {
                bitmap: &mut |paint| {
                    Some(Image {
                        image: ImageSource::opaque_id(ImageId::new(0)),
                        sampler: paint.sampler,
                    })
                },
                bound: &|_| true,
            },
        );
        self.replay.maintain();
        stats.draws + stats.glyph_runs
    }
}

/// glifo's glyph-run backend over a `vello_gpu` scene, with the outline
/// cache [`Replay`] keeps and the glyph atlas off: every glyph is drawn as
/// its outline.
struct Glyphs<'a> {
    scene: &'a mut Scene,
    cache: &'a mut GlyphPrepCache,
}

impl<'a> GlyphRunBackend<'a> for Glyphs<'a> {
    fn atlas_cache(self, _enabled: bool) -> Self {
        self
    }

    fn fill_glyphs<Iter>(
        self,
        run: glifo::GlyphRun<'a>,
        glyphs: Iter,
    ) -> Result<(), GlyphRenderError>
    where
        Iter: Iterator<Item = glifo::Glyph> + Clone,
    {
        run.build(glyphs, self.cache.as_mut(), AtlasCacher::Disabled)
            .fill_glyphs(self.scene)
    }

    fn stroke_glyphs<Iter>(
        self,
        run: glifo::GlyphRun<'a>,
        glyphs: Iter,
    ) -> Result<(), GlyphRenderError>
    where
        Iter: Iterator<Item = glifo::Glyph> + Clone,
    {
        let mut glyph_run = run.build(glyphs, self.cache.as_mut(), AtlasCacher::Disabled);
        // glifo folds a uniform scale of the run's transform into the font
        // size and draws the larger outline without it, so the stroke is
        // widened by that scale: the width the transform would have given.
        let stroke = self.scene.stroke().clone();
        let mut adjusted = stroke.clone();
        adjusted.width *= glyph_run.stroke_adjustment();
        self.scene.set_stroke(adjusted);
        let result = glyph_run.stroke_glyphs(self.scene);
        self.scene.set_stroke(stroke);
        result
    }

    fn render_decoration<Iter>(
        self,
        run: glifo::GlyphRun<'a>,
        glyphs: Iter,
        x_range: RangeInclusive<f32>,
        baseline_y: f32,
        offset: f32,
        size: f32,
        buffer: f32,
    ) where
        Iter: Iterator<Item = glifo::Glyph> + Clone,
    {
        run.build(glyphs, self.cache.as_mut(), AtlasCacher::Disabled)
            .render_decoration(x_range, baseline_y, offset, size, buffer, self.scene);
    }
}

/// The bitmap bytes kept resident across renders, in the image atlas and in
/// bitmaps' own textures together, for every target on a [`SharedGpu`].
#[cfg(not(target_arch = "wasm32"))]
const MAX_BITMAP_BYTES: u64 = 128 * 1024 * 1024;
/// The bitmap bytes kept resident across renders, in the image atlas and in
/// bitmaps' own textures together, for every target on a [`SharedGpu`].
#[cfg(target_arch = "wasm32")]
const MAX_BITMAP_BYTES: u64 = 64 * 1024 * 1024;

/// The side of one page of `vello_gpu`'s image atlas. The atlas creates a
/// page the first time a bitmap needs one and frees none, so the first
/// bitmap drawn on a [`SharedGpu`] costs a whole page; at 2048 that page is 16 MiB of
/// RGBA8, where `vello_gpu`'s default 4096 makes it 64 MiB.
const ATLAS_PAGE_SIDE: u16 = 2048;

/// The most pages the atlas grows to: 64 MiB of bitmaps at
/// [`ATLAS_PAGE_SIDE`]. A bitmap that finds no room in them takes a texture
/// of its own ([`Bitmaps`]).
const ATLAS_PAGES: usize = 4;

/// The longest side, edge included, a bitmap is uploaded to the atlas with.
/// A longer one is a draw of its own however it is stored, and in the atlas
/// it would hold a large share of a page that is never freed; its own
/// texture goes when it does.
const MAX_ATLAS_BITMAP_SIDE: u16 = 1024;

/// The smallest side of an intermediate layer texture: `vello_gpu`'s
/// default, which its `LayersConfig` documentation gives as the choice for
/// mobile GPUs, where smaller intermediates cost less than the extra render
/// passes they need. Textures grow to the largest layer a render needs and
/// do not shrink.
const MIN_LAYER_SIDE: u16 = 512;

/// The image atlas the backend renders with: [`ATLAS_PAGES`] pages of
/// [`ATLAS_PAGE_SIDE`], created as bitmaps need them.
fn atlas_config() -> AtlasConfig {
    AtlasConfig {
        initial_atlas_count: 0,
        max_atlases: ATLAS_PAGES,
        atlas_size: (ATLAS_PAGE_SIDE, ATLAS_PAGE_SIDE),
        auto_grow: true,
        allocation_strategy: AllocationStrategy::FirstFit,
    }
}

/// The settings a renderer on `device` is built with, `atlas` its image
/// atlas.
///
/// A layer's intermediate texture is as large as the layer's clip, and a
/// render rejects a layer longer than [`LayersConfig::max_texture_size`]
/// outright (`vello_gpu`'s default is 4096): a page-sized clip kept as a
/// layer on a target 4097 px tall would fail the whole frame. A layer lies
/// within its target, and no target is longer than the device's longest
/// texture side, so that side is the bound: every layer any target can hold
/// renders. `vello_gpu` clamps the atlas and the layer sizes to the same
/// limit; [`Bitmaps`] reads the atlas back from here with that clamp
/// applied.
fn render_settings(device: &wgpu::Device, atlas: AtlasConfig) -> RenderSettings {
    let longest = u16::try_from(device.limits().max_texture_dimension_2d).unwrap_or(u16::MAX);
    RenderSettings {
        memory_settings: MemorySettings {
            image_atlas_config: AtlasConfig {
                atlas_size: (
                    atlas.atlas_size.0.min(longest),
                    atlas.atlas_size.1.min(longest),
                ),
                initial_atlas_count: atlas.initial_atlas_count.min(atlas.max_atlases),
                ..atlas
            },
            layers_config: LayersConfig {
                max_textures: None,
                min_texture_size: SizeU16::new(MIN_LAYER_SIDE.min(longest)),
                max_texture_size: SizeU16::new(longest),
            },
        },
        ..RenderSettings::default()
    }
}

/// One bitmap upload's identity: the pixels, and which axes carry the
/// repeated edge [`Bitmaps`] describes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BitmapKey {
    blob: u64,
    width: u32,
    height: u32,
    /// [`ImageFormat`] and [`ImageAlphaType`] by their `repr(u8)`: one blob
    /// read under another layout is other pixels.
    layout: (u8, u8),
    edge: (bool, bool),
    /// Drawn nearest-neighbour, and so from a texture of its own; see
    /// [`Bitmaps`].
    nearest: bool,
}

/// Where an uploaded bitmap lives.
enum Store {
    Atlas(ImageId),
    /// A texture of its own, drawn as an external texture.
    Texture {
        id: TextureId,
        view: wgpu::TextureView,
    },
}

/// One target's hold on a bitmap: the target's render generation that last
/// drew it.
#[derive(Clone, Copy)]
struct Pin {
    target: u64,
    generation: u64,
}

/// One uploaded bitmap.
struct Bitmap {
    store: Store,
    /// The uploaded size, the repeated edge included.
    width: u16,
    height: u16,
    opaque: bool,
    bytes: u64,
    /// The [`Bitmaps::stamp`] of the render that last drew it, by any
    /// target: the release order.
    last_drawn: u64,
    /// Per target that has drawn it, the last generation that did.
    pins: SmallVec<[Pin; 2]>,
}

impl Bitmap {
    fn image(&self, sampler: ImageSampler) -> Image {
        let image = match &self.store {
            Store::Atlas(id) => ImageSource::opaque_id_with_transparency_hint(*id, !self.opaque),
            Store::Texture { id, .. } => ImageSource::ExternalTexture {
                id: *id,
                source_region: RectU16::new(0, 0, self.width, self.height),
                may_have_transparency: !self.opaque,
            },
        };
        Image { image, sampler }
    }

    /// Records that `drawing`'s current render draws this bitmap, dropping
    /// the pins of targets no longer live.
    fn mark(&mut self, drawing: &mut Drawing<'_>, targets: &FxHashMap<u64, u64>) {
        let (generation, stamp) = drawing.tick.expect("a drawing render has ticked");
        self.last_drawn = stamp;
        self.pins
            .retain(|pin| pin.target != drawing.target && targets.contains_key(&pin.target));
        self.pins.push(Pin {
            target: drawing.target,
            generation,
        });
        if let Store::Texture { id, view } = &self.store {
            drawing.textures.push((*id, view.clone()));
        }
    }

    /// Whether a live target drew it in one of its last [`PINNED_RENDERS`]
    /// bitmap-drawing renders.
    fn pinned(&self, targets: &FxHashMap<u64, u64>) -> bool {
        self.pins.iter().any(|pin| {
            targets
                .get(&pin.target)
                .is_some_and(|&now| pin.generation + PINNED_RENDERS > now)
        })
    }
}

/// How many of a target's latest bitmap-drawing renders pin what they drew.
/// A frame is several renders of one target — its `filter: blur()` and
/// vector bakes, then the composite — so what the composite drew stays
/// pinned through the next frame's bakes. A render that draws no bitmap
/// (most bakes) is not one of them.
const PINNED_RENDERS: u64 = 4;

/// The rasterizer state every target on one device shares: `vello_gpu`'s
/// renderer with the resources paired to it, and the bitmaps uploaded to
/// them.
///
/// # Locking
///
/// Targets render from the threads that own them, and each fills a scene
/// of its own. The CPU half of a render — the list replayed into that
/// scene, every path flattened and turned into strips — holds no lock but a
/// short [`Self::bitmaps`] hold per bitmap draw, which converts no pixels
/// and uploads nothing. The GPU half holds [`Self::renderer`] throughout:
/// the uploads the replays have queued, `Renderer::render` (its layer
/// schedule and the command encoding), the releases, and the submit. Two
/// targets therefore strip their scenes in parallel and take turns only
/// for the GPU half. The renderer lock is always taken before the bitmaps
/// one.
pub(crate) struct SharedRaster {
    renderer: Mutex<Paired>,
    bitmaps: Mutex<Bitmaps>,
}

impl std::fmt::Debug for SharedRaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRaster").finish_non_exhaustive()
    }
}

/// A renderer and the resources it was created with, which `vello_gpu`
/// requires to be used together only.
struct Paired {
    renderer: Renderer,
    resources: Resources,
}

/// The bitmaps renders draw, uploaded once per [`BitmapKey`] and released
/// once no live target draws them.
///
/// # Upload
///
/// A bitmap is converted to the premultiplied RGBA8 `vello_gpu` samples
/// ([`pixmap`]) and uploaded to the image atlas, or, when it is longer than
/// [`MAX_ATLAS_BITMAP_SIDE`], drawn nearest-neighbour, or no atlas page has
/// room for it, to a texture of its own drawn as an external texture. The
/// key names the blob, its size, its layout and whether it is drawn
/// nearest-neighbour, so a blob drawn at another size, under another layout
/// or with the other filter is another upload, and pixels that change take a
/// new blob and so a new key; the old upload ages out below. A blob two
/// targets draw is one upload.
///
/// A nearest-neighbour draw (`image-rendering: pixelated`) reads the texel
/// a sample position falls in. `vello_gpu` adds an atlas upload's offset to
/// the position in f32 before it rounds, so a position within that
/// addition's rounding error of a texel edge — every exact tie, which CSS
/// gives no rule for and layout produces readily — would read the texel on
/// one side or the other depending on where in the atlas the bitmap was
/// placed, and so on what every other target uploaded before it. From a
/// texture of its own the offset is zero and the position is read as
/// computed.
///
/// `Renderer::upload_image` panics when the atlas has no room, and
/// `vello_gpu` keeps its atlas allocator private. So [`Bitmaps::atlas`] is a
/// second allocator of the same configuration that every atlas upload and
/// release goes through first: the allocator is deterministic, and with the
/// glyph atlas cache off nothing else allocates in `vello_gpu`'s, so both
/// make the same decisions as long as both see the same allocations and
/// releases in the same order. A replay allocates here, with no renderer
/// lock, and queues the upload in [`Bitmaps::pending`]; the next render by
/// any target makes the queued uploads, in order, before it renders or
/// releases anything ([`Bitmaps::flush`]). The two hand out the same image
/// ids, which debug builds check.
///
/// `vello_gpu` pads an image axis by clamping the sample position to
/// `size − 1` (`extend.wesl`) before its bilinear and bicubic taps take
/// their half-texel offset, so every position past the boundary between the
/// last two texels reads them half and half: the last texel's outer half and
/// the whole padded area come out a blend of two colours where
/// clamp-to-edge reads the last texel alone. So an axis drawn with
/// [`Extend::Pad`] is uploaded with its last column (row) repeated once
/// more: the clamp then lands on the image's own edge and every filter reads
/// the last texel there. A repeating axis keeps its exact period and is
/// uploaded as it is.
///
/// # Lifecycle
///
/// Each target counts its renders that draw a bitmap — its generations —
/// and a bitmap is pinned while some live target drew it in one of its
/// last [`PINNED_RENDERS`] generations; a target that goes away
/// ([`SparseBackend`]'s `Drop`) pins nothing more. Once a render's draws are
/// encoded, the unpinned uploads are released least recently drawn first
/// (by any target) until the resident bytes are back within the budget —
/// an atlas upload with `Renderer::destroy_image`, a texture by dropping
/// it. Renders whose own bitmaps exceed the budget keep them all while they
/// are pinned: the bound is on what is kept across renders.
///
/// `destroy_image` clears the freed atlas region with a pass in the
/// releasing render's encoder, and an atlas upload is a queued write, which
/// applies at the start of the next submit. So a release runs after every
/// draw of its render that sampled the region, and is submitted before the
/// renderer lock is let go; an upload that lands in the region is
/// allocated after the release and so made by a later render, whose submit
/// follows the clear.
pub(crate) struct Bitmaps {
    entries: FxHashMap<BitmapKey, Bitmap>,
    /// Bytes of every upload.
    bytes: u64,
    /// The allocations of `vello_gpu`'s image atlas, replayed (see above).
    atlas: ImageCache,
    /// The atlas uploads allocated in [`Self::atlas`] and not yet made in
    /// `vello_gpu`'s, in allocation order.
    pending: Vec<(ImageId, Pixmap)>,
    /// Each live target's latest generation.
    targets: FxHashMap<u64, u64>,
    /// How many renders, by any target, have drawn a bitmap.
    stamp: u64,
    budget: u64,
    /// The device's longest texture side.
    longest: u32,
}

/// What one render of one target records on the bitmaps it draws.
struct Drawing<'a> {
    target: u64,
    /// The target's generation count.
    generation: &'a mut u64,
    /// The render's generation and [`Bitmaps::stamp`], once it has drawn a
    /// bitmap.
    tick: Option<(u64, u64)>,
    /// Where the render's bitmaps drawn from textures of their own go, to be
    /// bound for it.
    textures: &'a mut Vec<(TextureId, wgpu::TextureView)>,
}

impl Bitmaps {
    /// No bitmaps yet, `budget` bytes to keep, `atlas` the configuration
    /// `vello_gpu`'s image atlas was built with and `longest` the device's
    /// longest texture side.
    fn new(budget: u64, atlas: AtlasConfig, longest: u32) -> Self {
        Self {
            entries: FxHashMap::default(),
            bytes: 0,
            atlas: ImageCache::new_with_config(atlas),
            pending: Vec::new(),
            targets: FxHashMap::default(),
            stamp: 0,
            budget,
            longest,
        }
    }

    /// The key `image` drawn under `sampler` is uploaded under.
    fn key(&self, image: &ImageData, sampler: ImageSampler) -> BitmapKey {
        // The repeated edge needs one texel more than the image: a texture
        // already as long as the device allows keeps the shader's clamp.
        BitmapKey {
            blob: image.data.id(),
            width: image.width,
            height: image.height,
            layout: (image.format as u8, image.alpha_type as u8),
            edge: (
                sampler.x_extend == Extend::Pad && image.width < self.longest,
                sampler.y_extend == Extend::Pad && image.height < self.longest,
            ),
            nearest: sampler.quality == ImageQuality::Low,
        }
    }

    /// The upload under `key`, marked drawn by `drawing`'s render — whose
    /// first bitmap draw this makes it one of its target's generations.
    fn hit(
        &mut self,
        key: &BitmapKey,
        sampler: ImageSampler,
        drawing: &mut Drawing<'_>,
    ) -> Option<Image> {
        if drawing.tick.is_none() {
            *drawing.generation += 1;
            self.stamp += 1;
            self.targets.insert(drawing.target, *drawing.generation);
            drawing.tick = Some((*drawing.generation, self.stamp));
        }
        let bitmap = self.entries.get_mut(key)?;
        bitmap.mark(drawing, &self.targets);
        Some(bitmap.image(sampler))
    }

    /// Adds the upload `bitmap` under `key`, drawn by `drawing`'s render,
    /// which [`Self::hit`] has ticked.
    fn insert(
        &mut self,
        key: BitmapKey,
        mut bitmap: Bitmap,
        sampler: ImageSampler,
        drawing: &mut Drawing<'_>,
    ) -> Image {
        bitmap.mark(drawing, &self.targets);
        self.bytes += bitmap.bytes;
        self.entries
            .entry(key)
            .insert_entry(bitmap)
            .get()
            .image(sampler)
    }

    /// Makes the queued atlas uploads in `vello_gpu`'s atlas, in the order
    /// [`Self::atlas`] allocated them.
    fn flush(
        &mut self,
        paired: &mut Paired,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let Paired {
            renderer,
            resources,
        } = paired;
        for (slot, pixmap) in self.pending.drain(..) {
            let id = renderer.upload_image(resources, device, queue, encoder, &pixmap);
            debug_assert_eq!(id, slot, "the replayed atlas allocates as vello_gpu's does");
        }
    }

    /// Releases the unpinned uploads, least recently drawn first, until the
    /// resident bytes are within the budget; the atlas ones by clearing
    /// their region in `encoder`.
    ///
    /// Called with no upload queued, so `vello_gpu`'s atlas has seen every
    /// allocation [`Self::atlas`] has when both release.
    fn release(&mut self, paired: &mut Paired, encoder: &mut wgpu::CommandEncoder) {
        debug_assert!(self.pending.is_empty(), "every upload is made first");
        while self.bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(_, bitmap)| !bitmap.pinned(&self.targets))
                .min_by_key(|(_, bitmap)| bitmap.last_drawn)
                .map(|(key, _)| *key);
            let Some(key) = victim else {
                break;
            };
            let bitmap = self
                .entries
                .remove(&key)
                .expect("the victim was just found in the table");
            self.bytes -= bitmap.bytes;
            if let Store::Atlas(id) = bitmap.store {
                self.atlas.deallocate(id);
                paired
                    .renderer
                    .destroy_image(&mut paired.resources, encoder, id);
            }
        }
    }
}

/// A texture of its own holding `pixmap`, drawn as an external texture.
fn own_texture(device: &wgpu::Device, queue: &wgpu::Queue, pixmap: &Pixmap) -> Store {
    let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terminus bitmap"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        pixmap.data_as_u8_slice(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    Store::Texture {
        id: TextureId(next_texture_id()),
        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
    }
}

impl SharedRaster {
    /// The renderer, its resources and the bitmap table on `device`.
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self::with_atlas(device, atlas_config())
    }

    /// The same, with `atlas` the renderer's image atlas.
    pub(crate) fn with_atlas(device: &wgpu::Device, atlas: AtlasConfig) -> Self {
        let settings = render_settings(device, atlas);
        // The target size here only seeds a config buffer; every render
        // names its own.
        let (renderer, resources) = Renderer::new_with(
            device,
            &RenderTargetConfig {
                format: wgpu::TextureFormat::Rgba8Unorm,
                width: 1,
                height: 1,
            },
            settings,
        );
        Self {
            renderer: Mutex::new(Paired {
                renderer,
                resources,
            }),
            bitmaps: Mutex::new(Bitmaps::new(
                MAX_BITMAP_BYTES,
                settings.memory_settings.image_atlas_config,
                device.limits().max_texture_dimension_2d,
            )),
        }
    }

    /// Locks the renderer and its resources for one render's GPU half.
    ///
    /// A poisoned lock is taken anyway, here and in [`Self::bitmaps`]: the
    /// panic it records was another target's, and the state it guards is
    /// consistent between the steps a holder takes.
    fn renderer(&self) -> MutexGuard<'_, Paired> {
        self.renderer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the bitmap table.
    fn bitmaps(&self) -> MutexGuard<'_, Bitmaps> {
        self.bitmaps.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The image drawing `image` under `sampler` in `drawing`'s render,
    /// converted and queued for upload on a miss; `None` when the image's
    /// pixels are unreadable.
    ///
    /// The conversion, and a texture of its own, are made with no lock
    /// held; another target may upload the same key meanwhile, and then its
    /// upload is drawn and this one dropped.
    fn bitmap(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ImageData,
        sampler: ImageSampler,
        drawing: &mut Drawing<'_>,
    ) -> Option<Image> {
        let key = {
            let mut bitmaps = self.bitmaps();
            let key = bitmaps.key(image, sampler);
            if let Some(image) = bitmaps.hit(&key, sampler, drawing) {
                return Some(image);
            }
            key
        };
        let (pixmap, opaque) = pixmap(image, key.edge)?;
        let (width, height) = (pixmap.width(), pixmap.height());
        let bytes = u64::from(width) * u64::from(height) * 4;
        let mut bitmaps = self.bitmaps();
        if let Some(image) = bitmaps.hit(&key, sampler, drawing) {
            return Some(image);
        }
        // `upload_image` allocates with no padding.
        let slot = (!key.nearest && width.max(height) <= MAX_ATLAS_BITMAP_SIDE)
            .then(|| bitmaps.atlas.allocate(width, height, 0).ok())
            .flatten();
        let store = if let Some(slot) = slot {
            bitmaps.pending.push((slot, pixmap));
            Store::Atlas(slot)
        } else {
            drop(bitmaps);
            let store = own_texture(device, queue, &pixmap);
            bitmaps = self.bitmaps();
            if let Some(image) = bitmaps.hit(&key, sampler, drawing) {
                return Some(image);
            }
            store
        };
        let bitmap = Bitmap {
            store,
            width,
            height,
            opaque,
            bytes,
            last_drawn: 0,
            pins: SmallVec::new(),
        };
        Some(bitmaps.insert(key, bitmap, sampler, drawing))
    }

    /// The GPU half of a render on `gpu`, whose rasterizer this is: `scene`,
    /// which a target has filled, drawn into `view` over `clear` with
    /// `bindings` bound, under the renderer lock — the queued uploads, the
    /// render, the releases and the submit.
    /// Notes the wait for the lock and the time it was held in `stats`.
    fn render(
        &self,
        gpu: &SharedGpu,
        scene: &Scene,
        view: &wgpu::TextureView,
        bindings: &TextureBindings,
        clear: Color,
        stats: &mut Stats,
    ) -> Result<(), String> {
        let SharedGpu { device, queue, .. } = gpu;
        let render_size = RenderSize {
            width: scene.width(),
            height: scene.height(),
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terminus vello_gpu frame"),
        });
        let waited = ClockInstant::now();
        let mut paired = self.renderer();
        let started = ClockInstant::now();
        stats.wait = started.duration_since(waited);
        self.bitmaps()
            .flush(&mut paired, device, queue, &mut encoder);
        let Paired {
            renderer,
            resources,
        } = &mut *paired;
        let result = renderer.render(
            scene,
            resources,
            device,
            queue,
            &mut encoder,
            &render_size,
            view,
            None,
            bindings,
            TargetInit::Clear(ClearSettings::Viewport {
                color: clear.convert(),
            }),
        );
        {
            // Uploads queued while this target rendered were allocated
            // before anything below is released, so they are made first.
            let mut bitmaps = self.bitmaps();
            bitmaps.flush(&mut paired, device, queue, &mut encoder);
            bitmaps.release(&mut paired, &mut encoder);
        }
        // Submitted under the lock: the releases' clears must reach the queue
        // before any upload a later render makes into the freed regions.
        gpu.submit([encoder.finish()]);
        drop(paired);
        stats.render = started.elapsed();
        result.map_err(|error| error.to_string())
    }
}

/// `image`'s pixels as the premultiplied RGBA8 a pixmap holds, with the last
/// column and the last row repeated once more on the axes `edge` names (see
/// [`Bitmaps`]), and whether every pixel is opaque.
fn pixmap(image: &ImageData, edge: (bool, bool)) -> Option<(Pixmap, bool)> {
    let bytes = image.data.data();
    let (width, height) = (image.width as usize, image.height as usize);
    let row = width * 4;
    let padded_width = width + usize::from(edge.0);
    let padded_height = height + usize::from(edge.1);
    let (Ok(pixmap_width), Ok(pixmap_height)) =
        (u16::try_from(padded_width), u16::try_from(padded_height))
    else {
        return None;
    };
    if width == 0 || height == 0 || bytes.len() < row * height {
        return None;
    }
    let mut data = Vec::with_capacity(padded_width * padded_height * 4);
    let mut opaque = true;
    for source in bytes[..row * height].chunks_exact(row) {
        for px in source.chunks_exact(4) {
            let [mut r, g, mut b, a] = [px[0], px[1], px[2], px[3]];
            if image.format == ImageFormat::Bgra8 {
                std::mem::swap(&mut r, &mut b);
            }
            opaque &= a == 255;
            let mut texel = [r, g, b, a];
            if image.alpha_type == ImageAlphaType::Alpha {
                let a = u16::from(a);
                for c in &mut texel[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
            data.extend_from_slice(&texel);
        }
        if edge.0 {
            data.extend_from_within(data.len() - 4..);
        }
    }
    if edge.1 {
        data.extend_from_within(data.len() - padded_width * 4..);
    }
    let metadata = PixelMetadata::new(ImageAlphaType::AlphaPremultiplied, !opaque);
    Some((
        Pixmap::from_parts(data, pixmap_width, pixmap_height, metadata),
        opaque,
    ))
}

/// The next target's name in the bitmaps' pins.
fn next_target() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl SparseBackend {
    /// A target's rasterizer on `gpu`.
    pub(crate) fn new(gpu: &Arc<SharedGpu>) -> Self {
        Self {
            gpu: Arc::clone(gpu),
            target: next_target(),
            generation: 0,
            scene: Scene::new(1, 1),
            replay: Replay::default(),
            bound: FxHashMap::default(),
            textures: Vec::new(),
            last: Stats::default(),
        }
    }

    /// Binds `bound` to every draw of `texture`, or with `None` unbinds it.
    pub(crate) fn bind(&mut self, texture: &Texture, bound: Option<wgpu::Texture>) {
        match bound {
            Some(bound) => {
                self.bound.insert(
                    texture.id(),
                    bound.create_view(&wgpu::TextureViewDescriptor::default()),
                );
            }
            None => {
                self.bound.remove(&texture.id());
            }
        }
    }

    /// Renders `list` into `view`, a `width`×`height` RGBA8 target cleared
    /// to `base_color` first.
    ///
    /// The list is replayed into this target's scene with no lock held but
    /// the bitmap table's, briefly, per bitmap draw; the shared renderer is
    /// locked for the upload, render, release and submit that follow (see
    /// [`SharedRaster`]).
    pub(crate) fn render(
        &mut self,
        list: &DisplayList,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<(), String> {
        let (w, h) = (width as u16, height as u16);
        let Self {
            gpu,
            target,
            generation,
            scene,
            replay: kept,
            bound,
            textures,
            last,
        } = self;
        let SharedGpu {
            device,
            queue,
            raster: shared,
            ..
        } = &**gpu;

        let started = ClockInstant::now();
        scene.reset_and_resize(w, h);
        // A layer blending over the root blends against what is under the
        // whole list, which on the target is the base colour. `vello_gpu`
        // renders a root that is a blend target into a transparent
        // intermediate and blits that over the cleared target, so the clear
        // colour is under the blend rather than in it: the base colour is
        // drawn as the scene's first fill instead, which over a transparent
        // clear composes to the same pixels the clear would have.
        let underlay = base_color.components[3] > 0.0 && list.blends_at_root();
        if underlay {
            scene.set_transform(Affine::IDENTITY);
            scene.set_fill_rule(Fill::NonZero);
            scene.set_paint(base_color);
            scene.reset_paint_transform();
            scene.fill_rect(&Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));
        }
        let clear = if underlay {
            Color::TRANSPARENT
        } else {
            base_color
        };
        textures.clear();
        let mut drawing = Drawing {
            target: *target,
            generation,
            tick: None,
            textures,
        };
        let mut stats = replay(
            scene,
            list,
            kept,
            &mut Pictures {
                bitmap: &mut |paint| {
                    shared.bitmap(device, queue, &paint.image, paint.sampler, &mut drawing)
                },
                bound: &|id| bound.contains_key(&id),
            },
        );
        kept.maintain();
        let mut bindings = TextureBindings::new();
        for (&id, view) in bound.iter() {
            bindings.insert(TextureId(id), view.clone());
        }
        for (id, view) in drawing.textures.drain(..) {
            bindings.insert(id, view);
        }
        stats.encode = started.elapsed();

        let result = shared.render(gpu, scene, view, &bindings, clear, &mut stats);
        *last = stats;
        result
    }
}

impl Drop for SparseBackend {
    /// Gives up this target's pins: what only it drew becomes releasable at
    /// the next render that runs over the budget.
    fn drop(&mut self) {
        self.gpu.raster.bitmaps().targets.remove(&self.target);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use vello_gpu::{AllocationStrategy, AtlasConfig};

    use super::{Bitmaps, PINNED_RENDERS, SharedRaster, SparseBackend, Store};
    use crate::kurbo::{Affine, Rect};
    use crate::paint::display_list::DisplayList;
    use crate::peniko::{
        BlendMode, Blob, Color, Extend, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
        ImageQuality, ImageSampler,
    };
    use crate::render::gpu::SharedGpu;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A stack of its own, a target's backend on it, and one `width` ×
    /// `height` texture read back after each render.
    struct Rig {
        gpu: Arc<SharedGpu>,
        backend: SparseBackend,
        target: wgpu::Texture,
        width: u32,
        height: u32,
    }

    impl Rig {
        fn new(width: u32, height: u32) -> Self {
            Self::with_atlas(width, height, super::atlas_config())
        }

        fn with_atlas(width: u32, height: u32, atlas: AtlasConfig) -> Self {
            Self::on(
                &SharedGpu::private(|device| SharedRaster::with_atlas(device, atlas)),
                width,
                height,
            )
        }

        /// A target's backend on `gpu`, with its own texture.
        fn on(gpu: &Arc<SharedGpu>, width: u32, height: u32) -> Self {
            let gpu = Arc::clone(gpu);
            let backend = SparseBackend::new(&gpu);
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sparse test target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            Self {
                gpu,
                backend,
                target,
                width,
                height,
            }
        }

        /// Renders `scene` over white and answers the target's pixel at
        /// each of `at`.
        fn render(&mut self, scene: &DisplayList) -> Pixels {
            let device = &self.gpu.device;
            let view = self
                .target
                .create_view(&wgpu::TextureViewDescriptor::default());
            self.backend
                .render(scene, &view, self.width, self.height, Color::WHITE)
                .expect("render");
            let row = self.width * 4;
            let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sparse test readback"),
                size: u64::from(padded * self.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sparse test readback copy"),
            });
            encoder.copy_texture_to_buffer(
                self.target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            );
            self.gpu.submit([encoder.finish()]);
            let slice = buffer.slice(..);
            let (sender, receiver) = flume::bounded(1);
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            receiver.recv().expect("map callback").expect("map");
            let mapped = slice.get_mapped_range().expect("mapped range");
            Pixels {
                width: self.width,
                rgba: mapped
                    .chunks_exact(padded as usize)
                    .flat_map(|line| line[..row as usize].iter().copied())
                    .collect(),
            }
        }
    }

    impl Rig {
        /// The stack's bitmap table.
        fn bitmaps(&self) -> std::sync::MutexGuard<'_, Bitmaps> {
            self.gpu.raster.bitmaps()
        }
    }

    /// One read-back target.
    struct Pixels {
        width: u32,
        rgba: Vec<u8>,
    }

    impl Pixels {
        fn at(&self, x: u32, y: u32) -> [u8; 4] {
            let at = ((y * self.width + x) * 4) as usize;
            self.rgba[at..at + 4].try_into().expect("four channels")
        }
    }

    /// A straight-alpha RGBA8 bitmap of `texels`, `width` wide.
    fn bitmap(width: u32, texels: &[[u8; 4]]) -> ImageData {
        let bytes: Vec<u8> = texels.iter().flatten().copied().collect();
        ImageData {
            data: Blob::new(Arc::new(bytes)),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height: u32::try_from(texels.len()).expect("a small bitmap") / width,
        }
    }

    /// Fills `rect` with `image`, placed by `brush`.
    fn draw(
        scene: &mut DisplayList,
        image: &ImageData,
        quality: ImageQuality,
        extend: Extend,
        brush: Affine,
        rect: Rect,
    ) {
        let brush_ref = ImageBrush {
            image,
            sampler: ImageSampler {
                x_extend: extend,
                y_extend: extend,
                quality,
                alpha: 1.0,
            },
        };
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            brush_ref,
            Some(brush),
            &rect,
        );
    }

    /// A bilinear draw padded past its last texel's centre reads that texel
    /// (clamp-to-edge) — not the half-and-half blend of the last two that
    /// `vello_gpu`'s own clamp gives — and a repeating one keeps its period.
    #[test]
    fn a_padded_axis_filters_to_the_last_texel_and_a_repeating_one_keeps_its_period() {
        let mut rig = Rig::new(64, 16);
        let image = bitmap(2, &[RED, BLUE]);
        let mut scene = DisplayList::new();
        // Two texels at 8 px each, padded out to 32 px.
        draw(
            &mut scene,
            &image,
            ImageQuality::Medium,
            Extend::Pad,
            Affine::scale(8.0),
            Rect::new(0.0, 0.0, 32.0, 8.0),
        );
        // The same two texels repeated across 32 px below.
        draw(
            &mut scene,
            &image,
            ImageQuality::Medium,
            Extend::Repeat,
            Affine::translate((0.0, 8.0)) * Affine::scale(8.0),
            Rect::new(0.0, 8.0, 32.0, 16.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!(pixels.at(1, 4), RED, "before the first texel's centre");
        assert_eq!(pixels.at(14, 4), BLUE, "past the last texel's centre");
        assert_eq!(pixels.at(28, 4), BLUE, "in the padded area");
        // Between the centres the filter mixes the two: pixel 7's centre is
        // 7.5 / 8 texels in, 0.4375 of the way from the first centre.
        let mid = pixels.at(7, 4);
        assert!(
            (i32::from(mid[0]) - 143).abs() <= 2 && (i32::from(mid[2]) - 112).abs() <= 2,
            "{mid:?}"
        );
        assert_eq!(pixels.at(17, 12), RED, "a repeat starts over");
        assert_eq!(pixels.at(30, 12), BLUE);
        assert_eq!(
            rig.bitmaps().entries.len(),
            2,
            "the padded and the repeating draw are two uploads of one blob",
        );
    }

    /// One blob drawn as two sizes is two uploads, each its own pixels.
    #[test]
    fn one_blob_at_two_sizes_is_two_uploads() {
        let mut rig = Rig::new(16, 16);
        let square = bitmap(2, &[RED, BLUE, BLUE, RED]);
        let row = ImageData {
            width: 4,
            height: 1,
            ..square.clone()
        };
        let mut scene = DisplayList::new();
        draw(
            &mut scene,
            &square,
            ImageQuality::Low,
            Extend::Pad,
            Affine::scale(4.0),
            Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        draw(
            &mut scene,
            &row,
            ImageQuality::Low,
            Extend::Pad,
            Affine::translate((0.0, 12.0)) * Affine::scale(4.0),
            Rect::new(0.0, 12.0, 16.0, 16.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!(
            [
                pixels.at(1, 1),
                pixels.at(5, 1),
                pixels.at(1, 5),
                pixels.at(5, 5)
            ],
            [RED, BLUE, BLUE, RED]
        );
        assert_eq!(
            [
                pixels.at(1, 13),
                pixels.at(5, 13),
                pixels.at(9, 13),
                pixels.at(13, 13)
            ],
            [RED, BLUE, BLUE, RED]
        );
        assert_eq!(rig.bitmaps().entries.len(), 2);
    }

    /// A nearest-neighbour draw samples a texture of its own, where no atlas
    /// offset is added to its sample positions, and the same blob drawn
    /// smoothly is an atlas upload of its own.
    #[test]
    fn a_nearest_neighbour_bitmap_draws_from_its_own_texture() {
        let mut rig = Rig::new(12, 6);
        let stripes = bitmap(4, &[RED, RED, BLUE, BLUE]);
        let mut scene = DisplayList::new();
        for (row, quality) in [ImageQuality::Low, ImageQuality::Medium]
            .into_iter()
            .enumerate()
        {
            let y = 3.0 * row as f64;
            draw(
                &mut scene,
                &stripes,
                quality,
                Extend::Pad,
                Affine::translate((0.0, y)) * Affine::scale(3.0),
                Rect::new(0.0, y, 12.0, y + 3.0),
            );
        }
        let pixels = rig.render(&scene);
        for y in [1, 4] {
            assert_eq!([pixels.at(1, y), pixels.at(10, y)], [RED, BLUE], "row {y}");
        }
        let mut stores: Vec<_> = rig
            .bitmaps()
            .entries
            .iter()
            .map(|(key, bitmap)| (key.nearest, matches!(bitmap.store, Store::Texture { .. })))
            .collect();
        stores.sort_unstable();
        assert_eq!(
            stores,
            [(false, false), (true, true)],
            "one upload per filter, the nearest-neighbour one in a texture of its own"
        );
    }

    /// Past the budget, the bitmaps a render did not draw are released least
    /// recently drawn first, and one drawn again is uploaded again — into the
    /// region the release cleared, without that clear reaching it.
    #[test]
    fn bitmaps_past_the_budget_are_released_and_upload_again_on_return() {
        let mut rig = Rig::new(8, 8);
        let red = bitmap(1, &[RED]);
        let blue = bitmap(1, &[BLUE]);
        // One padded 2×2 upload's bytes.
        rig.bitmaps().budget = 16;
        let frame = |image: &ImageData| {
            let mut scene = DisplayList::new();
            draw(
                &mut scene,
                image,
                ImageQuality::Medium,
                Extend::Pad,
                Affine::scale(8.0),
                Rect::new(0.0, 0.0, 8.0, 8.0),
            );
            scene
        };
        assert_eq!(rig.render(&frame(&red)).at(4, 4), RED);
        assert_eq!(rig.bitmaps().bytes, 16);

        // Red stays pinned through the renders after it, then goes; a
        // render drawing no bitmap is not one of them.
        for _ in 1..PINNED_RENDERS {
            assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
            let _ = rig.render(&DisplayList::new());
            assert_eq!(rig.bitmaps().entries.len(), 2, "red is pinned");
        }
        assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
        assert_eq!(rig.bitmaps().entries.len(), 1, "red was released");
        assert_eq!(rig.bitmaps().bytes, 16);

        for _ in 0..2 {
            for _ in 0..PINNED_RENDERS {
                assert_eq!(rig.render(&frame(&red)).at(4, 4), RED, "uploaded again");
            }
            for _ in 0..PINNED_RENDERS {
                assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
            }
        }
        assert_eq!(rig.bitmaps().entries.len(), 1);

        // A render drawing both keeps both, past the budget, for itself.
        let mut both = frame(&red);
        draw(
            &mut both,
            &blue,
            ImageQuality::Medium,
            Extend::Pad,
            Affine::IDENTITY,
            Rect::new(4.0, 0.0, 8.0, 8.0),
        );
        let pixels = rig.render(&both);
        assert_eq!(pixels.at(1, 4), RED);
        assert_eq!(pixels.at(6, 4), BLUE);
        assert_eq!(rig.bitmaps().entries.len(), 2);
        for _ in 0..PINNED_RENDERS {
            let _ = rig.render(&frame(&blue));
        }
        assert_eq!(
            rig.bitmaps().entries.len(),
            1,
            "and once it is unpinned, red goes",
        );
    }

    /// A bitmap longer than the atlas takes for bitmaps is drawn from a
    /// texture of its own, and released by dropping it.
    #[test]
    fn a_long_bitmap_draws_from_its_own_texture() {
        let mut rig = Rig::new(16, 4);
        let mut texels = vec![RED; 1100];
        texels[1099] = BLUE;
        let image = bitmap(1100, &texels);
        let mut scene = DisplayList::new();
        // The last eight texels at pixels 0..8, then the padded area.
        draw(
            &mut scene,
            &image,
            ImageQuality::Medium,
            Extend::Pad,
            Affine::translate((-1092.0, 0.0)),
            Rect::new(0.0, 0.0, 16.0, 4.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!(pixels.at(0, 0), RED);
        assert_eq!(pixels.at(7, 0), BLUE);
        assert_eq!(pixels.at(12, 0), BLUE, "padded past the end");
        assert!(matches!(
            rig.bitmaps()
                .entries
                .values()
                .next()
                .map(|bitmap| &bitmap.store),
            Some(Store::Texture { .. })
        ));
        rig.bitmaps().budget = 0;
        let other = bitmap(1, &[RED]);
        for _ in 0..PINNED_RENDERS {
            let mut scene = DisplayList::new();
            draw(
                &mut scene,
                &other,
                ImageQuality::Medium,
                Extend::Pad,
                Affine::IDENTITY,
                Rect::new(0.0, 0.0, 1.0, 1.0),
            );
            let _ = rig.render(&scene);
        }
        assert_eq!(rig.bitmaps().entries.len(), 1, "the long one went");
    }

    /// Bitmaps past what the atlas has room for draw from textures of their
    /// own, and one released from the atlas frees its room for the next.
    #[test]
    fn a_bitmap_the_atlas_has_no_room_for_draws_from_its_own_texture() {
        // One 64 px page: room for one padded 40 × 40 upload.
        let mut rig = Rig::with_atlas(
            128,
            48,
            AtlasConfig {
                initial_atlas_count: 0,
                max_atlases: 1,
                atlas_size: (64, 64),
                auto_grow: true,
                allocation_strategy: AllocationStrategy::FirstFit,
            },
        );
        let images = [RED, BLUE, [0, 255, 0, 255]].map(|texel| bitmap(40, &vec![texel; 1600]));
        let frame = |images: &[ImageData]| {
            let mut scene = DisplayList::new();
            for (index, image) in images.iter().enumerate() {
                let x = 40.0 * index as f64;
                draw(
                    &mut scene,
                    image,
                    ImageQuality::Medium,
                    Extend::Pad,
                    Affine::translate((x, 0.0)),
                    Rect::new(x, 0.0, x + 40.0, 40.0),
                );
            }
            scene
        };
        let pixels = rig.render(&frame(&images));
        assert_eq!(
            [pixels.at(20, 20), pixels.at(60, 20), pixels.at(100, 20)],
            [RED, BLUE, [0, 255, 0, 255]]
        );
        let stores = |rig: &Rig| {
            let mut stores: Vec<_> = rig
                .bitmaps()
                .entries
                .iter()
                .map(|(key, bitmap)| (key.blob, matches!(bitmap.store, Store::Atlas(_))))
                .collect();
            stores.sort_unstable();
            stores
                .into_iter()
                .map(|(_, atlas)| atlas)
                .collect::<Vec<_>>()
        };
        assert_eq!(stores(&rig), [true, false, false], "only the first fits");

        // Once red is released, the next upload takes its room.
        rig.bitmaps().budget = 0;
        for _ in 0..PINNED_RENDERS {
            let _ = rig.render(&frame(&images[1..2]));
        }
        assert_eq!(
            stores(&rig),
            [false],
            "red went, blue stayed in its texture"
        );
        let fresh = bitmap(40, &vec![RED; 1600]);
        let pixels = rig.render(&frame(&[fresh]));
        assert_eq!(pixels.at(20, 20), RED);
        assert!(
            rig.bitmaps()
                .entries
                .values()
                .any(|bitmap| matches!(bitmap.store, Store::Atlas(_))),
            "the freed room is reused",
        );
    }

    /// A 1 × 1 bitmap of `texel` on a blob of its own, filling `rect`, drawn
    /// from the atlas.
    fn swatch(scene: &mut DisplayList, texel: [u8; 4], rect: Rect) -> ImageData {
        let image = bitmap(1, &[texel]);
        draw(
            scene,
            &image,
            ImageQuality::Medium,
            Extend::Pad,
            Affine::translate((rect.x0, rect.y0)),
            rect,
        );
        image
    }

    /// Two targets on one stack, each on a thread of its own, draw bitmaps
    /// of their own — a fresh one every frame, past a budget that releases
    /// the old ones as they go — and one they both draw. Every frame of
    /// each shows its own pixels: the uploads one target's replay queues
    /// and the releases the other's render makes leave both atlases in
    /// step, and neither releases what the other is drawing.
    #[test]
    fn two_targets_on_two_threads_draw_their_own_bitmaps() {
        let gpu = SharedGpu::private(SharedRaster::new);
        // Room for three padded 2 × 2 uploads.
        gpu.raster.bitmaps().budget = 48;
        let common = bitmap(1, &[[0, 255, 0, 255]]);
        std::thread::scope(|scope| {
            for side in 0..2_u8 {
                let gpu = &gpu;
                let common = &common;
                scope.spawn(move || {
                    let mut rig = Rig::on(gpu, 8, 4);
                    for frame in 0..48_u8 {
                        let own = [side * 120, frame * 5, 255 - side * 120, 255];
                        let mut scene = DisplayList::new();
                        swatch(&mut scene, own, Rect::new(0.0, 0.0, 4.0, 4.0));
                        draw(
                            &mut scene,
                            common,
                            ImageQuality::Low,
                            Extend::Pad,
                            Affine::translate((4.0, 0.0)),
                            Rect::new(4.0, 0.0, 8.0, 4.0),
                        );
                        let pixels = rig.render(&scene);
                        assert_eq!(pixels.at(1, 1), own, "target {side}, frame {frame}");
                        assert_eq!(pixels.at(6, 1), [0, 255, 0, 255], "target {side}");
                    }
                });
            }
        });
        let bitmaps = gpu.raster.bitmaps();
        assert!(bitmaps.pending.is_empty(), "every queued upload was made");
        assert!(
            bitmaps.entries.len() <= 2 * PINNED_RENDERS as usize + 1,
            "the old swatches were released: {} resident",
            bitmaps.entries.len()
        );
    }

    /// A bitmap stays pinned by every live target that drew it, counted in
    /// that target's own renders, and a target that goes away pins nothing:
    /// another target's renders release what only it drew, and draw on
    /// from what they share.
    #[test]
    fn a_target_that_goes_away_releases_its_pins_and_nothing_else() {
        let gpu = SharedGpu::private(SharedRaster::new);
        gpu.raster.bitmaps().budget = 0;
        let mut first = Rig::on(&gpu, 8, 4);
        let mut second = Rig::on(&gpu, 8, 4);

        let mut scene = DisplayList::new();
        let only_first = swatch(&mut scene, RED, Rect::new(0.0, 0.0, 4.0, 4.0));
        let both = swatch(&mut scene, BLUE, Rect::new(4.0, 0.0, 8.0, 4.0));
        let pixels = first.render(&scene);
        assert_eq!([pixels.at(1, 1), pixels.at(6, 1)], [RED, BLUE]);

        let mut theirs = DisplayList::new();
        draw(
            &mut theirs,
            &both,
            ImageQuality::Medium,
            Extend::Pad,
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 8.0, 4.0),
        );
        // The second target's renders, however many, release nothing the
        // first drew in its last render.
        for _ in 0..2 * PINNED_RENDERS {
            assert_eq!(second.render(&theirs).at(1, 1), BLUE);
        }
        assert_eq!(
            gpu.raster.bitmaps().entries.len(),
            2,
            "both pinned by the first"
        );

        drop(first);
        assert_eq!(second.render(&theirs).at(1, 1), BLUE, "drawn on");
        let bitmaps = gpu.raster.bitmaps();
        assert_eq!(
            bitmaps
                .entries
                .keys()
                .map(|key| key.blob)
                .collect::<Vec<_>>(),
            [both.data.id()],
            "what only the first drew was released, what the second draws kept"
        );
        assert!(
            !bitmaps
                .entries
                .keys()
                .any(|key| key.blob == only_first.data.id())
        );
    }

    /// A layer longer than `vello_gpu`'s default intermediate texture side
    /// (4096) renders: the layer limit is the device's longest side.
    #[test]
    fn a_layer_taller_than_4096_px_renders() {
        let mut rig = Rig::new(8, 5000);
        let mut scene = DisplayList::new();
        let page = Rect::new(0.0, 0.0, 8.0, 5000.0);
        scene.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            0.5,
            Affine::IDENTITY,
            &page,
        );
        scene.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &page);
        scene.pop_layer();
        let pixels = rig.render(&scene);
        for y in [0, 2500, 4999] {
            let [r, g, b, a] = pixels.at(4, y);
            assert!(
                r.abs_diff(128) <= 1 && r == g && g == b && a == 255,
                "half black over white at {y}: {:?}",
                [r, g, b, a]
            );
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod replay_tests {
    use super::*;
    use crate::kurbo::{Cap, Join, Line, Stroke};
    use crate::peniko::color::palette::css::{BLACK, BLUE, RED};
    use crate::peniko::{self, BlendMode, Compose, Gradient, Mix};
    use crate::render::gpu::Headless;

    fn render(scene: &DisplayList, width: u32, height: u32) -> Vec<u8> {
        let mut gpu = Headless::new().expect("a usable GPU adapter is mandatory");
        gpu.render(scene, width, height, Color::WHITE)
            .expect("the scene renders")
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const OPAQUE_RED: [u8; 4] = [255, 0, 0, 255];
    const OPAQUE_BLUE: [u8; 4] = [0, 0, 255, 255];
    const OPAQUE_BLACK: [u8; 4] = [0, 0, 0, 255];

    fn hard_stop(at: f32) -> [peniko::ColorStop; 4] {
        [
            (0.0, RED).into(),
            (at, RED).into(),
            (at, BLUE).into(),
            (1.0, BLUE).into(),
        ]
    }

    #[test]
    fn a_closed_stroke_joins_back_to_its_start() {
        let mut scene = DisplayList::new();
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let stroke = Stroke::new(2.0).with_join(Join::Miter);
        scene.stroke(&stroke, Affine::translate((1.0, 1.0)), BLACK, None, &rect);
        // Mitred corners, and the 8 px hole left unpainted.
        let pixels = render(&scene, 12, 12);
        for (x, y) in [(0, 0), (11, 0), (0, 11), (11, 11), (5, 1)] {
            assert_eq!(pixel(&pixels, 12, x, y), OPAQUE_BLACK, "({x}, {y})");
        }
        for (x, y) in [(2, 2), (5, 5), (9, 9)] {
            assert_eq!(pixel(&pixels, 12, x, y), WHITE, "({x}, {y})");
        }
    }

    #[test]
    fn a_stroke_covers_its_offsets_and_its_caps() {
        for (cap, capped) in [(Cap::Butt, false), (Cap::Square, true)] {
            let mut scene = DisplayList::new();
            let line = Line::new((2.0, 4.0), (14.0, 4.0));
            let stroke = Stroke::new(2.0).with_caps(cap);
            scene.stroke(&stroke, Affine::IDENTITY, BLACK, None, &line);
            let pixels = render(&scene, 16, 8);
            assert_eq!(pixel(&pixels, 16, 5, 3), OPAQUE_BLACK);
            assert_eq!(pixel(&pixels, 16, 5, 4), OPAQUE_BLACK);
            assert_eq!(pixel(&pixels, 16, 5, 2), WHITE);
            assert_eq!(pixel(&pixels, 16, 1, 3) == OPAQUE_BLACK, capped, "{cap:?}");
        }
    }

    /// A dashed stroke reaches the rasterizer already dashed.
    #[test]
    fn a_dashed_stroke_leaves_its_gaps_unpainted() {
        let mut scene = DisplayList::new();
        let line = Line::new((0.0, 4.0), (16.0, 4.0));
        let stroke = Stroke::new(2.0).with_dashes(0.0, [4.0, 4.0]);
        scene.stroke(&stroke, Affine::IDENTITY, BLACK, None, &line);
        let pixels = render(&scene, 16, 8);
        for x in [1, 9] {
            assert_eq!(pixel(&pixels, 16, x, 3), OPAQUE_BLACK, "dash at {x}");
        }
        for x in [5, 13] {
            assert_eq!(pixel(&pixels, 16, x, 3), WHITE, "gap at {x}");
        }
    }

    #[test]
    fn a_clip_layer_bounds_the_fill_inside_it() {
        let mut scene = DisplayList::new();
        scene.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(2.0, 2.0, 6.0, 6.0),
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        scene.pop_layer();
        let pixels = render(&scene, 8, 8);
        assert_eq!(pixel(&pixels, 8, 1, 1), WHITE);
        assert_eq!(pixel(&pixels, 8, 3, 3), OPAQUE_RED);
        assert_eq!(pixel(&pixels, 8, 6, 6), WHITE);
    }

    /// A hard stop on a pixel edge splits the columns there: each pixel
    /// takes the colour at its centre.
    #[test]
    fn a_hard_stop_on_a_pixel_edge_splits_the_columns_there() {
        let mut scene = DisplayList::new();
        let gradient = Gradient::new_linear((0.0, 0.0), (20.0, 0.0)).with_stops(hard_stop(0.5));
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &gradient,
            None,
            &Rect::new(0.0, 0.0, 20.0, 2.0),
        );
        let pixels = render(&scene, 20, 2);
        assert_eq!(pixel(&pixels, 20, 9, 0), OPAQUE_RED);
        assert_eq!(pixel(&pixels, 20, 10, 0), OPAQUE_BLUE);
    }

    /// Past its end line a gradient pads, repeats or reflects as its extend
    /// says.
    #[test]
    fn a_gradient_extends_past_its_end_as_its_extend_says() {
        // A 10 px period, red over its first half: pixel 12's centre is a
        // quarter into the second period, pixel 17's three quarters.
        for (extend, at_12, at_17) in [
            (Extend::Pad, OPAQUE_BLUE, OPAQUE_BLUE),
            (Extend::Repeat, OPAQUE_RED, OPAQUE_BLUE),
            (Extend::Reflect, OPAQUE_BLUE, OPAQUE_RED),
        ] {
            let mut scene = DisplayList::new();
            let gradient = Gradient::new_linear((0.0, 0.0), (10.0, 0.0))
                .with_stops(hard_stop(0.5))
                .with_extend(extend);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                &gradient,
                None,
                &Rect::new(0.0, 0.0, 20.0, 2.0),
            );
            let pixels = render(&scene, 20, 2);
            assert_eq!(pixel(&pixels, 20, 2, 0), OPAQUE_RED, "{extend:?}");
            assert_eq!(pixel(&pixels, 20, 12, 0), at_12, "{extend:?}");
            assert_eq!(pixel(&pixels, 20, 17, 0), at_17, "{extend:?}");
        }
    }

    /// Every pixel of a conic gradient is painted, its centre's included.
    #[test]
    fn a_conic_gradient_paints_its_centre() {
        let mut scene = DisplayList::new();
        let gradient = Gradient::new_sweep((8.0, 8.0), 0.0, std::f32::consts::TAU)
            .with_stops([RED, BLUE, RED].as_slice())
            .with_extend(Extend::Repeat);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &gradient,
            None,
            &Rect::new(0.0, 0.0, 16.0, 16.0),
        );
        let pixels = render(&scene, 16, 16);
        for (x, y) in [(8, 8), (7, 7), (8, 7), (7, 8), (9, 9)] {
            assert_ne!(pixel(&pixels, 16, x, y), WHITE, "({x}, {y})");
        }
    }

    /// A layer blending over the root blends against the base colour.
    #[test]
    fn a_root_blend_reads_the_base_colour() {
        let mut scene = DisplayList::new();
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Difference, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 4.0, 4.0),
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(0.0, 0.0, 4.0, 4.0),
        );
        scene.pop_layer();
        assert!(scene.blends_at_root());
        // |white − red| is cyan.
        assert_eq!(pixel(&render(&scene, 4, 4), 4, 1, 1), [0, 255, 255, 255]);
    }

    /// A non-separable blend inside a rounded clip reads full coverage along
    /// the clip's anti-aliased rim: desaturating white over white leaves
    /// white everywhere, rim included, with no dark pixel (the
    /// `backdrop-filter: grayscale()` pass `draw_backdrop` draws).
    #[test]
    fn a_saturation_blend_in_a_rounded_clip_leaves_the_rim_clean() {
        let shape = crate::kurbo::RoundedRect::new(2.0, 2.0, 62.0, 30.0, 13.5);
        let bounds = Rect::new(2.0, 2.0, 62.0, 30.0);
        let mut scene = DisplayList::new();
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &shape);
        scene.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &bounds);
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Saturation, Compose::SrcAtop),
            1.0,
            Affine::IDENTITY,
            &bounds,
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(128, 128, 128),
            None,
            &bounds,
        );
        scene.pop_layer();
        scene.pop_layer();
        let pixels = render(&scene, 64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let px = pixel(&pixels, 64, x, y);
                assert!(
                    px.iter().all(|&c| c >= 254),
                    "({x}, {y}) is {px:?}, not white"
                );
            }
        }
    }

    /// A blend directly inside a rounded clip blends at full coverage with
    /// what the clip holds and is cut once, by the clip: multiplying red into
    /// blue gives black, and the rim is that black faded over the white page,
    /// with no red or blue of the operands showing through. No layer stands
    /// between the clip and the blend: the list isolates the clip.
    #[test]
    fn a_multiply_blend_in_a_rounded_clip_is_cut_after_it_blends() {
        let shape = crate::kurbo::RoundedRect::new(0.0, 0.0, 32.0, 32.0, 12.0);
        let bounds = Rect::new(0.0, 0.0, 32.0, 32.0);
        let mut scene = DisplayList::new();
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &shape);
        scene.fill(Fill::NonZero, Affine::IDENTITY, BLUE, None, &bounds);
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Multiply, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &bounds,
        );
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &bounds);
        scene.pop_layer();
        scene.pop_layer();
        let pixels = render(&scene, 32, 32);
        assert_eq!(pixel(&pixels, 32, 16, 16), OPAQUE_BLACK);
        assert_eq!(pixel(&pixels, 32, 0, 0), WHITE);
        let mut partial = 0;
        for y in 0..32 {
            for x in 0..32 {
                let [r, g, b, a] = pixel(&pixels, 32, x, y);
                assert!(
                    r.abs_diff(g) <= 1 && b.abs_diff(g) <= 1 && a == 255,
                    "({x}, {y}) is {:?}, not a grey",
                    [r, g, b, a]
                );
                partial += usize::from(g != 0 && g != 255);
            }
        }
        assert!(partial > 0, "the rounded rim is anti-aliased");
    }

    /// An inset shadow's shape: a clip holding a fill and a `DestOut` layer
    /// that erases a hole from it. The erase reaches only what the clip
    /// holds — the page painted before the clip shows through the hole —
    /// because the list composites the clip as an isolated layer.
    #[test]
    fn a_destination_out_in_a_clip_erases_only_what_the_clip_holds() {
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        let page = Rect::new(0.0, 0.0, 16.0, 16.0);
        let clip = Rect::new(2.0, 2.0, 14.0, 14.0);
        let mut scene = DisplayList::new();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(0, 255, 0),
            None,
            &page,
        );
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &clip);
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::DestOut),
            1.0,
            Affine::IDENTITY,
            &clip,
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::WHITE,
            None,
            &Rect::new(6.0, 6.0, 10.0, 10.0),
        );
        scene.pop_layer();
        scene.pop_layer();
        let pixels = render(&scene, 16, 16);
        assert_eq!(pixel(&pixels, 16, 0, 0), GREEN, "the page outside the clip");
        assert_eq!(pixel(&pixels, 16, 3, 3), OPAQUE_RED, "the clip's fill");
        assert_eq!(pixel(&pixels, 16, 8, 8), GREEN, "the hole shows the page");
    }
}
