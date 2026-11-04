//! The `vello_gpu` rasterizer: a composed [`vello::Scene`] replayed into a
//! `vello_gpu::Scene` and rendered by sparse strips on `vello_gpu`'s wgpu
//! backend. Compiled with the `vello_gpu` feature; [`super::raster::Raster`]
//! is its only caller.
//!
//! The paint and compose layers build a `vello::Scene` either way. This
//! module decodes its encoding stream by stream — paths, styles,
//! transforms, draw objects and the late-bound patches (ramps, images, glyph
//! runs) — and replays it into a `vello_gpu::Scene` of the target's size.
//! An image whose handle has a texture bound ([`SparseBackend::bind`]: the
//! `filter: blur()`, `backdrop-filter` and vector-image bakes) is drawn as an
//! external texture, sampled in place; any other bitmap is uploaded once and
//! released once renders stop drawing it ([`Bitmaps`]).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    reason = "a decoder over vello's packed streams: counts and offsets are small exact integers"
)]

use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;
use vello::kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Stroke};
use vello::low_level::vello_encoding::{
    DrawBeginClip, DrawTag, Encoding, Patch, PathTag, Style, Transform,
};
use vello::peniko::{
    self, BlendMode, Color, Compose, Extend, Fill, Gradient, ImageAlphaType, ImageData,
    ImageFormat, ImageQuality, ImageSampler, Mix,
};
use vello::wgpu;
use vello_gpu::{
    AtlasWriter, ClearSettings, Image, ImageId, ImageSource, PixelMetadata, Pixmap, RectU16,
    RenderSize, RenderTargetConfig, Renderer, Resources, Scene, TargetInit, TextureBindings,
    TextureId,
};

/// What one frame cost, and what it could not draw.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Stats {
    /// Decoding the vello encoding and building the sparse-strip scene
    /// (path flattening and strip generation run here, on the CPU).
    pub(crate) encode: Duration,
    /// Of `encode`, decoding vello's streams alone (the trial's own cost).
    pub(crate) decode: Duration,
    /// `Renderer::render` and the submit.
    pub(crate) render: Duration,
    pub(crate) draws: u32,
    pub(crate) glyph_runs: u32,
    pub(crate) layers: u32,
    /// Image draws with neither a bound texture nor readable pixels.
    pub(crate) images_skipped: u32,
    pub(crate) masks_skipped: u32,
}

pub(crate) struct SparseBackend {
    renderer: Renderer,
    resources: Resources,
    scene: Scene,
    /// Uploaded bitmaps, with their lifecycle.
    images: Bitmaps,
    /// Textures bound to image handles, by blob id.
    bound: FxHashMap<u64, Bound>,
    next_texture: u64,
    pub(crate) last: Stats,
}

/// A texture bound to an image handle.
struct Bound {
    id: TextureId,
    view: wgpu::TextureView,
    width: u16,
    height: u16,
}

impl std::fmt::Debug for SparseBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparseBackend")
            .field("last", &self.last)
            .finish()
    }
}

/// One decoded path: segments in the coordinates of `transform`.
struct DecodedPath {
    path: BezPath,
    transform: Affine,
    /// The transform current at the path's end marker: the brush's.
    brush_transform: Affine,
    style: Style,
}

fn affine(transform: &Transform) -> Affine {
    let [a, b, c, d] = transform.matrix;
    let [e, f] = transform.translation;
    Affine::new([
        f64::from(a),
        f64::from(b),
        f64::from(c),
        f64::from(d),
        f64::from(e),
        f64::from(f),
    ])
}

fn is_stroke(style: &Style) -> bool {
    style.flags_and_miter_limit & Style::FLAGS_STYLE_BIT != 0
}

fn fill_rule(style: &Style) -> Fill {
    if style.flags_and_miter_limit & Style::FLAGS_FILL_BIT == 0 {
        Fill::NonZero
    } else {
        Fill::EvenOdd
    }
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        0x1f => sign * f32::INFINITY,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

fn stroke(style: &Style) -> Stroke {
    let flags = style.flags_and_miter_limit;
    let join = match flags & Style::FLAGS_JOIN_MASK {
        Style::FLAGS_JOIN_BITS_MITER => Join::Miter,
        Style::FLAGS_JOIN_BITS_ROUND => Join::Round,
        _ => Join::Bevel,
    };
    let cap = |bits: u32| match bits {
        Style::FLAGS_CAP_BITS_SQUARE => Cap::Square,
        Style::FLAGS_CAP_BITS_ROUND => Cap::Round,
        _ => Cap::Butt,
    };
    Stroke::new(f64::from(style.line_width))
        .with_join(join)
        .with_start_cap(cap((flags & Style::FLAGS_START_CAP_MASK) >> 2))
        .with_end_cap(cap(flags & Style::FLAGS_END_CAP_MASK))
        .with_miter_limit(f64::from(f16_to_f32(
            (flags & Style::MITER_LIMIT_MASK) as u16,
        )))
}

/// Every path of `encoding`, in path order, as `PathMonoid` counts them.
fn decode_paths(encoding: &Encoding) -> Vec<DecodedPath> {
    let mut out = Vec::with_capacity(encoding.n_paths as usize);
    let mut transform = Affine::IDENTITY;
    let mut style = Style::from_fill(Fill::NonZero);
    let mut transforms = encoding.transforms.iter();
    let mut styles = encoding.styles.iter();
    // Point offset of the next segment's first point, in points.
    let mut point = 0_usize;
    let mut word = 0_usize;
    let mut path = BezPath::new();
    let mut path_transform: Option<Affine> = None;
    // The open subpath of a stroke, so its cap marker can be dropped.
    let mut subpath: Vec<vello::kurbo::PathEl> = Vec::new();
    let mut subpath_open = false;
    let read = |word: usize, is_f32: bool| -> Point {
        if is_f32 {
            Point::new(
                f64::from(f32::from_bits(encoding.path_data[word])),
                f64::from(f32::from_bits(encoding.path_data[word + 1])),
            )
        } else {
            let w = encoding.path_data[word];
            Point::new(f64::from((w & 0xffff) as i16), f64::from((w >> 16) as i16))
        }
    };
    for tag in &encoding.path_tags {
        let t = tag.0;
        if t & PathTag::TRANSFORM.0 != 0 {
            if let Some(next) = transforms.next() {
                transform = affine(next);
            }
        }
        if t & PathTag::STYLE.0 != 0 {
            if let Some(next) = styles.next() {
                style = *next;
            }
        }
        let segment = t & 0x3;
        if segment != 0 {
            let is_f32 = t & 0x8 != 0;
            let size = if is_f32 { 2 } else { 1 };
            let _ = point;
            if path_transform.is_none() {
                path_transform = Some(transform);
            }
            let p: Vec<Point> = (0..=segment as usize)
                .map(|k| read(word + k * size, is_f32))
                .collect();
            if !subpath_open {
                subpath.push(vello::kurbo::PathEl::MoveTo(p[0]));
                subpath_open = true;
            }
            subpath.push(match segment {
                1 => vello::kurbo::PathEl::LineTo(p[1]),
                2 => vello::kurbo::PathEl::QuadTo(p[1], p[2]),
                _ => vello::kurbo::PathEl::CurveTo(p[1], p[2], p[3]),
            });
            let end = (t >> 2) & 1;
            let advance = segment as usize + end as usize;
            point += advance;
            word += advance * size;
            if end != 0 {
                if is_stroke(&style) {
                    // The subpath's last segment is vello's cap marker: a
                    // line for a closed subpath, a quad for an open one.
                    let closed = matches!(subpath.pop(), Some(vello::kurbo::PathEl::LineTo(_)));
                    for el in subpath.drain(..) {
                        path.push(el);
                    }
                    if closed {
                        path.close_path();
                    }
                } else {
                    for el in subpath.drain(..) {
                        path.push(el);
                    }
                    path.close_path();
                }
                subpath_open = false;
            }
        }
        if t & PathTag::PATH.0 != 0 {
            out.push(DecodedPath {
                path: std::mem::take(&mut path),
                transform: path_transform.take().unwrap_or(transform),
                brush_transform: transform,
                style,
            });
        }
    }
    out
}

fn premul_color(rgba: u32) -> Color {
    let [r, g, b, a] = rgba.to_ne_bytes();
    if a == 0 {
        return Color::TRANSPARENT;
    }
    let un = |c: u8| ((f32::from(c) * 255.0 / f32::from(a)).round().min(255.0)) as u8;
    Color::from_rgba8(un(r), un(g), un(b), a)
}

fn extend(bits: u32) -> Extend {
    match bits & 0x3 {
        1 => Extend::Repeat,
        2 => Extend::Reflect,
        _ => Extend::Pad,
    }
}

fn mix(value: u32) -> Mix {
    match value {
        1 => Mix::Multiply,
        2 => Mix::Screen,
        3 => Mix::Overlay,
        4 => Mix::Darken,
        5 => Mix::Lighten,
        6 => Mix::ColorDodge,
        7 => Mix::ColorBurn,
        8 => Mix::HardLight,
        9 => Mix::SoftLight,
        10 => Mix::Difference,
        11 => Mix::Exclusion,
        12 => Mix::Hue,
        13 => Mix::Saturation,
        14 => Mix::Color,
        15 => Mix::Luminosity,
        _ => Mix::Normal,
    }
}

fn compose(value: u32) -> Compose {
    match value {
        0 => Compose::Clear,
        1 => Compose::Copy,
        2 => Compose::Dest,
        4 => Compose::DestOver,
        5 => Compose::SrcIn,
        6 => Compose::DestIn,
        7 => Compose::SrcOut,
        8 => Compose::DestOut,
        9 => Compose::SrcAtop,
        10 => Compose::DestAtop,
        11 => Compose::Xor,
        12 => Compose::Plus,
        13 => Compose::PlusLighter,
        _ => Compose::SrcOver,
    }
}

/// The bitmap bytes kept resident across renders, in the image atlas and in
/// bitmaps' own textures together.
#[cfg(not(target_arch = "wasm32"))]
const MAX_BITMAP_BYTES: u64 = 128 * 1024 * 1024;
/// The bitmap bytes kept resident across renders, in the image atlas and in
/// bitmaps' own textures together.
#[cfg(target_arch = "wasm32")]
const MAX_BITMAP_BYTES: u64 = 64 * 1024 * 1024;

/// Of those, the bytes that may live in `vello_gpu`'s image atlas at once.
/// The atlas shares its 4096 × 4096 layers with the glyph cache, creates them
/// as it fills and frees none, and `Renderer::upload_image` panics when no
/// layer has room; so bitmaps take at most one layer's worth of it and the
/// rest take their own textures.
const MAX_ATLAS_BITMAP_BYTES: u64 = 64 * 1024 * 1024;

/// The longest side, edge included, a bitmap is uploaded to the atlas with.
/// A longer one is a draw of its own however it is stored, and in the atlas
/// it would hold a large share of a layer that is never freed; its own
/// texture goes when it does.
const MAX_ATLAS_BITMAP_SIDE: u16 = 1024;

/// How far a nearest-neighbour sample position is moved along each image
/// axis, in texels.
///
/// `vello_gpu` forms an image sample position at the corners of a strip's
/// quad and the rasterizer interpolates it, so a pixel centre that maps
/// exactly onto a texel boundary comes out an ulp or two to either side of
/// it. vello evaluates the position per pixel, and the exact value is on the
/// boundary, which nearest-neighbour sampling gives to the texel after it.
/// 2^-12 texel is several ulps of any texel coordinate below 512, and far
/// below the distance a pixel centre that is not on a boundary keeps from one
/// at any ordinary scale.
const TIE_NUDGE: f64 = 1.0 / 4096.0;

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

/// One uploaded bitmap.
struct Bitmap {
    store: Store,
    /// The uploaded size, the repeated edge included.
    width: u16,
    height: u16,
    opaque: bool,
    bytes: u64,
    /// The render that last drew it; see [`Bitmaps::tick`].
    last_used: u64,
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
}

/// How many of the latest bitmap-drawing renders pin what they drew. A frame
/// is several renders through one backend — its `filter: blur()` and vector
/// bakes, then the composite — so what the composite drew stays pinned
/// through the next frame's bakes; vello's own image cache spares what the
/// last two of its renders drew the same way. A render that draws no bitmap
/// (most bakes) is not a tick at all.
const PINNED_RENDERS: u64 = 4;

/// The bitmaps a render draws, uploaded once per [`BitmapKey`] and released
/// once no render draws them.
///
/// # Upload
///
/// A bitmap is converted to the premultiplied RGBA8 `vello_gpu` samples
/// ([`pixmap`]) and uploaded to the shared image atlas, or, when it is longer
/// than [`MAX_ATLAS_BITMAP_SIDE`] or the atlas already holds
/// [`MAX_ATLAS_BITMAP_BYTES`] of bitmaps, to a texture of its own drawn as an
/// external texture. The key names the blob, its size and its layout, so a
/// blob drawn at another size or under another layout is another upload, and
/// pixels that change take a new blob and so a new key; the old upload ages
/// out below.
///
/// `vello_gpu` pads an image axis by clamping the sample position to the last
/// texel's *centre* before it filters, where vello clamps it to the image's
/// edge: a bilinear or bicubic sample past the last centre blends the last
/// two texels half and half — a half-texel band along the right and bottom
/// edge of every magnified bitmap, and the whole padded area beyond it. So an
/// axis drawn with [`Extend::Pad`] is uploaded with its last column (row)
/// repeated once more: the clamp then lands on the repeated texel's centre,
/// the image's own edge, and every filter reads what vello reads. A repeating
/// axis keeps its exact period and is uploaded as it is.
///
/// # Lifecycle
///
/// Every render that draws a bitmap is one tick, and what the last
/// [`PINNED_RENDERS`] of them drew is pinned. Once a render's draws are
/// encoded, the unpinned uploads are released least recently drawn first
/// until the resident bytes are back within [`MAX_BITMAP_BYTES`] — an atlas
/// upload with `Renderer::destroy_image`, a texture by dropping it. Renders
/// whose own bitmaps exceed the budget keep them all while they are pinned:
/// the bound is on what is kept across renders, and resident bytes never
/// exceed the larger of the budget and what the pinned renders drew.
pub(crate) struct Bitmaps {
    entries: FxHashMap<BitmapKey, Bitmap>,
    /// Bytes of every upload, and of those in the atlas.
    bytes: u64,
    atlas_bytes: u64,
    /// How many renders have drawn a bitmap. An upload's `last_used` is the
    /// tick of the render that last drew it, so those within
    /// [`PINNED_RENDERS`] of the current tick are the pinned ones and the
    /// smallest tick is the next to go.
    tick: u64,
    /// Whether the current render has drawn a bitmap, and so is a tick.
    drawing: bool,
    budget: u64,
    /// The device's longest texture side, read at the first upload.
    longest: Option<u32>,
}

impl Bitmaps {
    fn with_budget(budget: u64) -> Self {
        Self {
            entries: FxHashMap::default(),
            bytes: 0,
            atlas_bytes: 0,
            tick: 0,
            drawing: false,
            budget,
            longest: None,
        }
    }

    /// Starts a render; its first bitmap draw makes it a tick.
    fn begin(&mut self) {
        self.drawing = false;
    }

    /// The paint drawing `image` under `sampler`, uploaded on a miss; `None`
    /// when the image's pixels are unreadable.
    #[expect(
        clippy::too_many_arguments,
        reason = "an upload's device handles beside the texture ids it draws from"
    )]
    fn paint(
        &mut self,
        renderer: &mut Renderer,
        resources: &mut Resources,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        next_texture: &mut u64,
        image: &ImageData,
        sampler: ImageSampler,
    ) -> Option<Image> {
        // The repeated edge needs one texel more than the image: a texture
        // already as long as the device allows keeps the shader's clamp.
        let longest = *self
            .longest
            .get_or_insert_with(|| device.limits().max_texture_dimension_2d);
        let key = BitmapKey {
            blob: image.data.id(),
            width: image.width,
            height: image.height,
            layout: (image.format as u8, image.alpha_type as u8),
            edge: (
                sampler.x_extend == Extend::Pad && image.width < longest,
                sampler.y_extend == Extend::Pad && image.height < longest,
            ),
        };
        if !self.drawing {
            self.drawing = true;
            self.tick += 1;
        }
        let tick = self.tick;
        if let Some(bitmap) = self.entries.get_mut(&key) {
            bitmap.last_used = tick;
            return Some(bitmap.image(sampler));
        }
        let (pixmap, opaque) = pixmap(image, key.edge)?;
        let (width, height) = (pixmap.width(), pixmap.height());
        let bytes = u64::from(width) * u64::from(height) * 4;
        let store = if width.max(height) <= MAX_ATLAS_BITMAP_SIDE
            && self.atlas_bytes + bytes <= MAX_ATLAS_BITMAP_BYTES
        {
            self.atlas_bytes += bytes;
            Store::Atlas(renderer.upload_image(resources, device, queue, encoder, &pixmap))
        } else {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terminus vello_gpu bitmap"),
                size: wgpu::Extent3d {
                    width: u32::from(width),
                    height: u32::from(height),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            pixmap.write_to_atlas(device, queue, encoder, &texture, [0, 0], width, height);
            let id = TextureId(*next_texture);
            *next_texture += 1;
            Store::Texture {
                id,
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            }
        };
        self.bytes += bytes;
        let bitmap = self.entries.entry(key).or_insert(Bitmap {
            store,
            width,
            height,
            opaque,
            bytes,
            last_used: tick,
        });
        Some(bitmap.image(sampler))
    }

    /// Binds the textures of the bitmaps the current render draws from their
    /// own texture.
    fn bind_into(&self, bindings: &mut TextureBindings) {
        for bitmap in self.entries.values() {
            if let Store::Texture { id, view } = &bitmap.store
                && self.drawing
                && bitmap.last_used == self.tick
            {
                bindings.insert(*id, view.clone());
            }
        }
    }

    /// Ends a render: releases the unpinned uploads, least recently drawn
    /// first, until the resident bytes are within the budget.
    ///
    /// Called once the render's draws are encoded into `encoder` and before
    /// it is submitted. `destroy_image` clears the freed atlas region with a
    /// pass in `encoder`, so the clear runs after every draw that sampled the
    /// region, and an upload that lands there later — a queued write, which
    /// applies at the start of the submit after this one — after the clear.
    /// Releasing inside the render instead would let this render's own
    /// uploads, applied before its encoder runs, be cleared by it.
    fn finish(
        &mut self,
        renderer: &mut Renderer,
        resources: &mut Resources,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        while self.bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(_, bitmap)| bitmap.last_used + PINNED_RENDERS <= self.tick)
                .min_by_key(|(_, bitmap)| bitmap.last_used)
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
                self.atlas_bytes -= bitmap.bytes;
                renderer.destroy_image(resources, encoder, id);
            }
        }
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

/// The sampler a `DrawImage`'s packed sample word names.
fn sampler(sample: u32) -> ImageSampler {
    let quality = match (sample >> 12) & 0x3 {
        0 => ImageQuality::Low,
        1 => ImageQuality::Medium,
        _ => ImageQuality::High,
    };
    ImageSampler {
        x_extend: extend(sample >> 10),
        y_extend: extend(sample >> 8),
        quality,
        alpha: (sample & 0xff) as f32 / 255.0,
    }
}

/// The paint drawing `image`, and the image-space map to compose onto the
/// right of its paint transform: its bound texture when it has one, else its
/// pixels ([`Bitmaps`]); the map is [`TIE_NUDGE`]'s, for a nearest-neighbour
/// draw.
#[expect(
    clippy::too_many_arguments,
    reason = "the upload's device handles beside the two caches it consults"
)]
fn image_paint(
    images: &mut Bitmaps,
    bound: &FxHashMap<u64, Bound>,
    renderer: &mut Renderer,
    resources: &mut Resources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    next_texture: &mut u64,
    image: &ImageData,
    sample: u32,
) -> Option<(Image, Affine)> {
    let sampler = sampler(sample);
    // Composed on the right, the map moves the image by −nudge in its own
    // space, so every sample position by +nudge.
    let nudge = if sampler.quality == ImageQuality::Low {
        Affine::translate((-TIE_NUDGE, -TIE_NUDGE))
    } else {
        Affine::IDENTITY
    };
    if let Some(texture) = bound.get(&image.data.id()) {
        // A texture longer than its handle carries the repeated edge a vector
        // bake adds (`vector_textures`), sampled on a padded axis only.
        let region = |extend: Extend, size: u32, texture: u16| {
            let size = u16::try_from(size).map_or(texture, |size| size.min(texture));
            if extend == Extend::Pad && texture > size {
                size + 1
            } else {
                size
            }
        };
        let paint = Image {
            image: ImageSource::ExternalTexture {
                id: texture.id,
                source_region: RectU16::new(
                    0,
                    0,
                    region(sampler.x_extend, image.width, texture.width),
                    region(sampler.y_extend, image.height, texture.height),
                ),
                may_have_transparency: true,
            },
            sampler,
        };
        return Some((paint, nudge));
    }
    let paint = images.paint(
        renderer,
        resources,
        device,
        queue,
        encoder,
        next_texture,
        image,
        sampler,
    )?;
    Some((paint, nudge))
}

impl SparseBackend {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        // The target size here only seeds a config buffer; every render
        // names its own.
        let (renderer, resources) = Renderer::new(
            device,
            &RenderTargetConfig {
                format: wgpu::TextureFormat::Rgba8Unorm,
                width: 1,
                height: 1,
            },
        );
        Self {
            renderer,
            resources,
            scene: Scene::new(1, 1),
            images: Bitmaps::with_budget(MAX_BITMAP_BYTES),
            bound: FxHashMap::default(),
            next_texture: 0,
            last: Stats::default(),
        }
    }

    /// Binds `texture` to every draw of `handle`, or with `None` unbinds it.
    pub(crate) fn bind(&mut self, handle: &ImageData, texture: Option<wgpu::Texture>) {
        let key = handle.data.id();
        match texture {
            Some(texture) => {
                let id = TextureId(self.next_texture);
                self.next_texture += 1;
                self.bound.insert(
                    key,
                    Bound {
                        id,
                        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                        width: texture.width() as u16,
                        height: texture.height() as u16,
                    },
                );
            }
            None => {
                self.bound.remove(&key);
            }
        }
    }

    /// Renders `vello_scene` into `view`, a `width`×`height` RGBA8 target
    /// cleared to `base_color` first.
    #[expect(
        clippy::too_many_arguments,
        reason = "one render's full inputs: the device pair, the scene, and the target with its \
                  size and base colour"
    )]
    pub(crate) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vello_scene: &vello::Scene,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<(), String> {
        let (w, h) = (width as u16, height as u16);
        let Self {
            renderer,
            resources,
            scene,
            images,
            bound,
            next_texture,
            last,
        } = self;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terminus vello_gpu frame"),
        });
        images.begin();

        let started = Instant::now();
        let mut stats = Stats::default();
        scene.reset_and_resize(w, h);
        let encoding = vello_scene.encoding();
        let paths = decode_paths(encoding);
        stats.decode = started.elapsed();
        let mut paths = paths.into_iter();
        let ramps: FxHashMap<usize, &Patch> = encoding
            .resources
            .patches
            .iter()
            .filter_map(|patch| match patch {
                Patch::Ramp {
                    draw_data_offset, ..
                }
                | Patch::Image {
                    draw_data_offset, ..
                } => Some((*draw_data_offset, patch)),
                Patch::GlyphRun { .. } => None,
            })
            .collect();
        let runs: FxHashMap<usize, usize> = encoding
            .resources
            .glyph_runs
            .iter()
            .enumerate()
            .map(|(index, run)| (run.stream_offsets.draw_tags, index))
            .collect();
        let data = &encoding.draw_data;
        let mut offset = 0_usize;
        let f = |at: usize| f32::from_bits(data[at]);
        for (index, tag) in encoding.draw_tags.iter().enumerate() {
            let size = ((tag.0 >> 2) & 0x7) as usize;
            let at = offset;
            offset += size;
            if *tag == DrawTag::NOP {
                continue;
            }
            let glyph_run = runs.get(&index).copied();
            let decoded = if glyph_run.is_some() {
                None
            } else {
                paths.next()
            };
            // An image paint's own image-space map (`image_paint`).
            let mut nudge = Affine::IDENTITY;
            // The paint of this draw, and its brush transform.
            let paint: Option<vello_gpu::PaintType> = match *tag {
                DrawTag::COLOR => Some(premul_color(data[at]).into()),
                DrawTag::LINEAR_GRADIENT | DrawTag::RADIAL_GRADIENT | DrawTag::SWEEP_GRADIENT => {
                    let Some(Patch::Ramp {
                        stops,
                        extend,
                        interpolation_alpha_space,
                        ..
                    }) = ramps.get(&at)
                    else {
                        continue;
                    };
                    let p = |i: usize| Point::new(f64::from(f(at + i)), f64::from(f(at + i + 1)));
                    let mut gradient = match *tag {
                        DrawTag::LINEAR_GRADIENT => Gradient::new_linear(p(1), p(3)),
                        DrawTag::RADIAL_GRADIENT => {
                            Gradient::new_two_point_radial(p(1), f(at + 5), p(3), f(at + 6))
                        }
                        _ => Gradient::new_sweep(
                            p(1),
                            f(at + 3) * std::f32::consts::TAU,
                            f(at + 4) * std::f32::consts::TAU,
                        ),
                    };
                    gradient.extend = *extend;
                    gradient.interpolation_alpha_space = *interpolation_alpha_space;
                    gradient.stops = encoding.resources.color_stops[stops.clone()].into();
                    Some(gradient.into())
                }
                DrawTag::IMAGE => {
                    let Some(Patch::Image { image, .. }) = ramps.get(&at) else {
                        continue;
                    };
                    let paint = image_paint(
                        images,
                        bound,
                        renderer,
                        resources,
                        device,
                        queue,
                        &mut encoder,
                        next_texture,
                        image,
                        data[at + 2],
                    );
                    if paint.is_none() {
                        stats.images_skipped += 1;
                    }
                    paint.map(|(paint, map)| {
                        nudge = map;
                        paint.into()
                    })
                }
                DrawTag::BLUR_RECT => Some(premul_color(data[at]).into()),
                _ => None,
            };

            if let Some(run_index) = glyph_run {
                let run = &encoding.resources.glyph_runs[run_index];
                let Some(paint) = paint else { continue };
                stats.glyph_runs += 1;
                scene.set_paint(paint);
                scene.set_transform(affine(&run.transform));
                match run.brush_transform {
                    Some(t) => scene.set_paint_transform(affine(&t)),
                    None => scene.reset_paint_transform(),
                }
                let glyphs = encoding.resources.glyphs[run.glyphs.clone()]
                    .iter()
                    .map(|g| glifo::Glyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                    });
                let coords = &encoding.resources.normalized_coords[run.normalized_coords.clone()];
                let mut builder = scene
                    .glyph_run(resources, &run.font)
                    .font_size(run.font_size)
                    .hint(run.hint)
                    .normalized_coords(coords);
                let embolden = glifo::FontEmbolden {
                    amount: run.font_embolden.amount,
                    join: run.font_embolden.join,
                    miter_limit: run.font_embolden.miter_limit,
                    tolerance: run.font_embolden.tolerance,
                };
                builder = builder.font_embolden(embolden);
                // vello applies the per-glyph transform in the outline's
                // y-up space; glifo in the scene's y-down one.
                let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
                let glyph_transform = run.glyph_transform.map(|t| flip * affine(&t) * flip);
                if let Some(t) = glyph_transform {
                    builder = builder.glyph_transform(t);
                }
                let result = match &run.style {
                    peniko::Style::Fill(_) => builder.fill_glyphs(glyphs),
                    peniko::Style::Stroke(stroke) => {
                        let stroke = stroke.clone();
                        drop(builder);
                        scene.set_stroke(stroke);
                        let mut builder = scene
                            .glyph_run(resources, &run.font)
                            .font_size(run.font_size)
                            .hint(run.hint)
                            .normalized_coords(coords)
                            .font_embolden(embolden);
                        if let Some(t) = glyph_transform {
                            builder = builder.glyph_transform(t);
                        }
                        builder.stroke_glyphs(glyphs)
                    }
                };
                let _ = result;
                continue;
            }

            let Some(decoded) = decoded else { continue };
            scene.set_transform(decoded.transform);
            scene.set_fill_rule(fill_rule(&decoded.style));
            match *tag {
                DrawTag::BEGIN_CLIP => {
                    stats.layers += 1;
                    let blend = data[at];
                    let alpha = f(at + 1);
                    if blend == DrawBeginClip::CLIP_BLEND_MODE {
                        scene.push_clip_layer(&decoded.path);
                    } else {
                        if blend == DrawBeginClip::LUMINANCE_MASK_BLEND_MODE {
                            stats.masks_skipped += 1;
                        }
                        let mode = BlendMode::new(mix((blend >> 8) & 0xff), compose(blend & 0xff));
                        scene.push_layer(Some(&decoded.path), Some(mode), Some(alpha), None, None);
                    }
                }
                DrawTag::END_CLIP => scene.pop_layer(),
                DrawTag::BLUR_RECT => {
                    stats.draws += 1;
                    if let Some(paint) = paint {
                        scene.set_paint(paint);
                    }
                    scene.reset_paint_transform();
                    scene.set_transform(decoded.brush_transform);
                    let (width, height) = (f64::from(f(at + 1)), f64::from(f(at + 2)));
                    scene.fill_blurred_rounded_rect(
                        &Rect::from_center_size(Point::ORIGIN, (width, height)),
                        f(at + 3),
                        f(at + 4),
                        false,
                    );
                }
                _ => {
                    let Some(paint) = paint else { continue };
                    stats.draws += 1;
                    let solid = matches!(paint, vello_gpu::PaintType::Solid(_));
                    scene.set_paint(paint);
                    if solid {
                        scene.reset_paint_transform();
                    } else {
                        scene.set_paint_transform(
                            decoded.transform.inverse() * decoded.brush_transform * nudge,
                        );
                    }
                    if is_stroke(&decoded.style) {
                        scene.set_stroke(stroke(&decoded.style));
                        scene.stroke_path(&decoded.path);
                    } else {
                        scene.fill_path(&decoded.path);
                    }
                }
            }
        }
        stats.encode = started.elapsed();

        let started = Instant::now();
        let render_size = RenderSize {
            width: w,
            height: h,
        };
        let mut bindings = TextureBindings::new();
        for texture in bound.values() {
            bindings.insert(texture.id, texture.view.clone());
        }
        images.bind_into(&mut bindings);
        let result = renderer.render(
            scene,
            resources,
            device,
            queue,
            &mut encoder,
            &render_size,
            view,
            None,
            &bindings,
            TargetInit::Clear(ClearSettings::Viewport {
                color: base_color.convert(),
            }),
        );
        images.finish(renderer, resources, &mut encoder);
        queue.submit([encoder.finish()]);
        stats.render = started.elapsed();
        *last = stats;
        result.map_err(|error| error.to_string())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use vello::kurbo::{Affine, Rect};
    use vello::peniko::{
        Blob, Color, Extend, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
        ImageQuality, ImageSampler,
    };
    use vello::util::RenderContext;
    use vello::wgpu;

    use super::{PINNED_RENDERS, SparseBackend, Store};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A device, a backend on it, and one `width` × `height` target read back
    /// after each render.
    struct Rig {
        context: RenderContext,
        device: usize,
        backend: SparseBackend,
        target: wgpu::Texture,
        width: u32,
        height: u32,
    }

    impl Rig {
        fn new(width: u32, height: u32) -> Self {
            let mut context = RenderContext::new();
            let device = pollster::block_on(context.device(None))
                .expect("a usable GPU adapter is mandatory");
            let handle = &context.devices[device];
            let backend = SparseBackend::new(&handle.device);
            let target = handle.device.create_texture(&wgpu::TextureDescriptor {
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
                context,
                device,
                backend,
                target,
                width,
                height,
            }
        }

        /// Renders `scene` over white and answers the target's pixel at
        /// each of `at`.
        fn render(&mut self, scene: &vello::Scene) -> Pixels {
            let handle = &self.context.devices[self.device];
            let (device, queue) = (&handle.device, &handle.queue);
            let view = self
                .target
                .create_view(&wgpu::TextureViewDescriptor::default());
            self.backend
                .render(
                    device,
                    queue,
                    scene,
                    &view,
                    self.width,
                    self.height,
                    Color::WHITE,
                )
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
            queue.submit([encoder.finish()]);
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
        scene: &mut vello::Scene,
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

    /// A bilinear draw padded past its last texel's centre reads that texel,
    /// as vello does — not the half-and-half blend of the last two that
    /// `vello_gpu`'s own clamp gives — and a repeating one keeps its period.
    #[test]
    fn a_padded_axis_filters_to_the_last_texel_and_a_repeating_one_keeps_its_period() {
        let mut rig = Rig::new(64, 16);
        let image = bitmap(2, &[RED, BLUE]);
        let mut scene = vello::Scene::new();
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
            rig.backend.images.entries.len(),
            2,
            "the padded and the repeating draw are two uploads of one blob",
        );
    }

    /// A nearest-neighbour pixel centre exactly on a texel boundary takes the
    /// texel after it, the exact value's.
    #[test]
    fn a_nearest_sample_on_a_texel_boundary_takes_the_texel_after_it() {
        let mut rig = Rig::new(128, 4);
        let image = bitmap(8, &[RED, RED, BLUE, BLUE, BLUE, BLUE, RED, RED]);
        let mut scene = vello::Scene::new();
        // At 13.75 px per texel, pixel 27's centre is exactly texel 2's
        // left edge and pixel 82's texel 6's: an object-fit golden's scale.
        draw(
            &mut scene,
            &image,
            ImageQuality::Low,
            Extend::Pad,
            Affine::scale(13.75),
            Rect::new(0.0, 0.0, 110.0, 4.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!([pixels.at(26, 0), pixels.at(27, 0)], [RED, BLUE]);
        assert_eq!([pixels.at(81, 0), pixels.at(82, 0)], [BLUE, RED]);
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
        let mut scene = vello::Scene::new();
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
        assert_eq!(rig.backend.images.entries.len(), 2);
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
        rig.backend.images.budget = 16;
        let frame = |image: &ImageData| {
            let mut scene = vello::Scene::new();
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
        assert_eq!(rig.backend.images.bytes, 16);

        // Red stays pinned through the renders after it, then goes; a
        // render drawing no bitmap is not one of them.
        for _ in 1..PINNED_RENDERS {
            assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
            let _ = rig.render(&vello::Scene::new());
            assert_eq!(rig.backend.images.entries.len(), 2, "red is pinned");
        }
        assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
        assert_eq!(rig.backend.images.entries.len(), 1, "red was released");
        assert_eq!(rig.backend.images.bytes, 16);
        assert_eq!(rig.backend.images.atlas_bytes, 16);

        for _ in 0..2 {
            for _ in 0..PINNED_RENDERS {
                assert_eq!(rig.render(&frame(&red)).at(4, 4), RED, "uploaded again");
            }
            for _ in 0..PINNED_RENDERS {
                assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
            }
        }
        assert_eq!(rig.backend.images.entries.len(), 1);

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
        assert_eq!(rig.backend.images.entries.len(), 2);
        for _ in 0..PINNED_RENDERS {
            let _ = rig.render(&frame(&blue));
        }
        assert_eq!(
            rig.backend.images.entries.len(),
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
        let mut scene = vello::Scene::new();
        // The last eight texels at pixels 0..8, then the padded area.
        draw(
            &mut scene,
            &image,
            ImageQuality::Low,
            Extend::Pad,
            Affine::translate((-1092.0, 0.0)),
            Rect::new(0.0, 0.0, 16.0, 4.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!(pixels.at(0, 0), RED);
        assert_eq!(pixels.at(7, 0), BLUE);
        assert_eq!(pixels.at(12, 0), BLUE, "padded past the end");
        assert!(matches!(
            rig.backend
                .images
                .entries
                .values()
                .next()
                .map(|bitmap| &bitmap.store),
            Some(Store::Texture { .. })
        ));
        assert_eq!(rig.backend.images.atlas_bytes, 0);
        rig.backend.images.budget = 0;
        let other = bitmap(1, &[RED]);
        for _ in 0..PINNED_RENDERS {
            let mut scene = vello::Scene::new();
            draw(
                &mut scene,
                &other,
                ImageQuality::Low,
                Extend::Pad,
                Affine::IDENTITY,
                Rect::new(0.0, 0.0, 1.0, 1.0),
            );
            let _ = rig.render(&scene);
        }
        assert_eq!(rig.backend.images.entries.len(), 1, "the long one went");
    }
}
