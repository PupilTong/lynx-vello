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
//! external texture, sampled in place; any other bitmap is uploaded to
//! `vello_gpu`'s image atlas once per blob.

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
    ClearSettings, Image, ImageId, ImageSource, PixelMetadata, Pixmap, RectU16, RenderSize,
    RenderTargetConfig, Renderer, Resources, Scene, TargetInit, TextureBindings, TextureId,
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
    /// Uploaded bitmaps, by blob id and size.
    images: FxHashMap<(u64, u32, u32), ImageId>,
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

/// Whether some layer of `encoding` that the root holds blends by anything
/// but source-over.
///
/// vello starts every pixel of the root at the base colour, so such a layer
/// blends against it. `vello_gpu` renders a root that is a blend target into
/// a transparent intermediate and blits that over the cleared target, so the
/// clear colour is under the blend rather than in it; [`SparseBackend::render`]
/// then draws the base colour as the scene's first fill instead.
fn blends_over_root(encoding: &Encoding) -> bool {
    let mut depth = 0_u32;
    let mut offset = 0_usize;
    for tag in &encoding.draw_tags {
        let at = offset;
        offset += ((tag.0 >> 2) & 0x7) as usize;
        match *tag {
            DrawTag::BEGIN_CLIP => {
                let blend = encoding.draw_data[at];
                if depth == 0
                    && blend != DrawBeginClip::CLIP_BLEND_MODE
                    && BlendMode::new(mix((blend >> 8) & 0xff), compose(blend & 0xff))
                        != BlendMode::default()
                {
                    return true;
                }
                depth += 1;
            }
            DrawTag::END_CLIP => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// `image`'s pixels as the premultiplied RGBA8 a pixmap holds.
fn pixmap(image: &ImageData) -> Option<Pixmap> {
    let bytes = image.data.data();
    let (width, height) = (image.width, image.height);
    if bytes.len() < (width * height * 4) as usize || width > u32::from(u16::MAX) {
        return None;
    }
    let mut data = bytes[..(width * height * 4) as usize].to_vec();
    for px in data.chunks_exact_mut(4) {
        if image.format == ImageFormat::Bgra8 {
            px.swap(0, 2);
        }
        if image.alpha_type == ImageAlphaType::Alpha {
            let a = u16::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Some(Pixmap::from_parts(
        data,
        width as u16,
        height as u16,
        PixelMetadata::default(),
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

/// The paint drawing `image`: its bound texture when it has one, else its
/// pixels, uploaded once per blob.
#[expect(
    clippy::too_many_arguments,
    reason = "the upload's device handles beside the two caches it consults"
)]
fn image_paint(
    images: &mut FxHashMap<(u64, u32, u32), ImageId>,
    bound: &FxHashMap<u64, Bound>,
    renderer: &mut Renderer,
    resources: &mut Resources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    image: &ImageData,
    sample: u32,
) -> Option<Image> {
    if let Some(texture) = bound.get(&image.data.id()) {
        return Some(Image {
            image: ImageSource::ExternalTexture {
                id: texture.id,
                source_region: RectU16::new(0, 0, texture.width, texture.height),
                may_have_transparency: true,
            },
            sampler: sampler(sample),
        });
    }
    let key = (image.data.id(), image.width, image.height);
    let id = if let Some(id) = images.get(&key) {
        *id
    } else {
        let pixmap = pixmap(image)?;
        let id = renderer.upload_image(resources, device, queue, encoder, &pixmap);
        images.insert(key, id);
        id
    };
    Some(Image {
        image: ImageSource::opaque_id(id),
        sampler: sampler(sample),
    })
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
            images: FxHashMap::default(),
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
            last,
            ..
        } = self;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terminus vello_gpu frame"),
        });

        let started = Instant::now();
        let mut stats = Stats::default();
        scene.reset_and_resize(w, h);
        let encoding = vello_scene.encoding();
        // See `blends_over_root`: the base colour as a fill is what a root
        // blend has to read, and over a transparent clear it composes to the
        // same pixels the clear would have.
        let underlay = base_color.components[3] > 0.0 && blends_over_root(encoding);
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
                        image,
                        data[at + 2],
                    );
                    if paint.is_none() {
                        stats.images_skipped += 1;
                    }
                    paint.map(Into::into)
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
                            decoded.transform.inverse() * decoded.brush_transform,
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
                color: clear.convert(),
            }),
        );
        queue.submit([encoder.finish()]);
        stats.render = started.elapsed();
        *last = stats;
        result.map_err(|error| error.to_string())
    }
}
