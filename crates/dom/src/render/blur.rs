#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "device-pixel counts, kernel weights and uniform-block offsets convert between \
              u32/u64/usize and f32/f64 throughout; every value is bounded by \
              MAX_FILTER_DIMENSION, MAX_TAPS, MAX_LEVELS or PARAMS_SIZE"
)]

//! The offscreen pass behind CSS `filter: blur()`.
//!
//! A commit records each blurred group as a [`crate::FilterGroup`] — σ in
//! device px, the device rect to bake (already 3σ larger than the group's own
//! bounds), and the program ops that make up the group's content. Nothing in
//! that record is a GPU resource, so the commit stays device-free and the
//! frame stays `Send + Sync`. [`FilterTextures`] is where the device side
//! lives: one per [`vello::Renderer`], owned beside that renderer's
//! [`AtlasResidency`], and asked once per
//! rendered frame for the table of baked textures the compose program indexes
//! by group.
//!
//! # One pre-step, cached
//!
//! [`FilterTextures::prepare`] is the whole of it, and it is skipped
//! altogether for a frame with no filter group — which is every frame of a
//! page that does not blur. Its cache key is the commit id, plus the
//! painter's scroll generation when (and only when) some group's content
//! rides a scroll chain the group itself does not: a blurred scroller's
//! *content* moves under the blur, so its bake depends on the offset, while
//! an ordinary blurred box moves with it and its bake does not. So a scroll
//! frame over an ordinary blurred box re-bakes nothing.
//!
//! **That key identifies a commit of *one* document.** Commit ids restart at
//! one per document, so a consumer pointing this renderer at a second
//! document has to call [`FilterTextures::forget`] first — the same
//! obligation, for the same reason, that it already has for its own compose
//! key.
//!
//! # The pass chain
//!
//! Per group, in this order:
//!
//! 1. **Bake.** The group's ops replay into a scratch [`vello::Scene`] with the group's own chain
//!    factored out (see [`crate::CommittedFrame::bake_filter`]) and render into a `STORAGE_BINDING`
//!    target over `Color::TRANSPARENT`.
//! 2. **Premultiply.** vello writes its target *unpremultiplied*, and filtering unpremultiplied
//!    color pulls the color of fully transparent pixels into their neighbours — a white square on
//!    white gets a dark halo. So the first pass is `vec4(rgb * a, a)`, and the whole rest of the
//!    chain is premultiplied.
//! 3. **Decimate.** While the remaining variance exceeds 4 (σ > 2), take a 2× box downsample and
//!    account for it: `V = (V − 0.917) / 4`. The 0.917 is the variance the resample pair itself
//!    contributes in source px² — 0.25 for the box downsample plus 0.667 for the bilinear tent that
//!    brings it back up.
//! 4. **Blur.** One separable gaussian at `σ_k` = √V ≤ 2, radius `ceil(3σ_k)` ≤ 6, horizontal then
//!    vertical, each half-pass ≤ 7 texture fetches thanks to the linear-sampling pair trick. The
//!    horizontal pass writes into the deepest level's `pong`, or — with no decimation, where the
//!    bake target has already been consumed — straight back into the bake target.
//! 5. **Interpolate back.** 2× bilinear upsamples to level 0. The last one writes the output
//!    texture.
//!
//! The output is bound to a stable `peniko::ImageData` handle through
//! `Renderer::override_image`, declared `AlphaPremultiplied` so vello's
//! `fine.wgsl` samples it without premultiplying a second time, and marked
//! dirty once per bake so the atlas re-copies exactly the frames it has to.
//!
//! # Budget
//!
//! Filter memory is page-complexity-linear, so it is capped:
//! [`MAX_FILTER_DIMENSION`] per texture side and [`MAX_FILTER_AREA`] summed
//! over a frame's groups, consumed in program order. A group past the cap
//! gets no texture, and the compose program's documented fallback takes over:
//! its ops replay raw and that group renders **unblurred** rather than not at
//! all. One admitted group costs three RGBA8 textures of its own area — vello's
//! bake target, the premultiplied level 0, and the output — plus, when it
//! decimates, the under-⅓-area pyramid and one plane at the deepest level; see
//! `Bank`. At four bytes a pixel and at most four full-res planes' worth per
//! group, the cap therefore bounds a frame's whole filter memory at about
//! 256 MiB — a number only a pathological page approaches, and in practice
//! vello's 8192² atlas binds first.

use euclid::default::Vector2D;
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use vello::wgpu;

use super::gpu::{AtlasResidency, GpuError, render_params};
use crate::visual::{CommittedFrame, ScrollSlot};

/// Largest per-axis device-pixel count one filter bake may have.
///
/// vello's image atlas stops doubling at 8192 px, so a texture longer than
/// this on either axis can never be placed in it at any atlas size — the
/// same bound [`crate::MAX_RENDERABLE_DIMENSION`] enforces for bitmaps.
pub const MAX_FILTER_DIMENSION: u32 = 8192;

/// Largest total device-pixel area one frame's filter bakes may cover.
///
/// A quarter of the atlas at its largest, so blurred groups cannot crowd out
/// the page's own bitmaps. See the module doc for what a group over the cap
/// does instead.
pub const MAX_FILTER_AREA: u64 = (MAX_FILTER_DIMENSION as u64 * MAX_FILTER_DIMENSION as u64) / 4;

/// Variance, in source px², that one 2× box downsample and the bilinear tent
/// that undoes it contribute together: 0.25 from the box (two samples one px
/// apart), 0.667 from the tent.
const RESAMPLE_VARIANCE: f64 = 0.917;

/// Variance above which one more decimation level is worth taking — σ = 2,
/// which is exactly the largest σ the ≤ 7-tap kernel covers to 3σ.
const DECIMATE_ABOVE: f64 = 4.0;

/// Decimation levels a bake may take.
///
/// This is a guard, not the stop that normally fires: the size floor in
/// [`decimate`] halts a `MAX_FILTER_DIMENSION`-wide rect after 12 levels
/// anyway (8192 halves to 2 in twelve steps). It has to be at least that
/// large to be inert, because the largest σ a bake rect can carry is the one
/// whose own 6σ extent fills it — σ ≈ 8192/6 ≈ 1365 — and twelve levels
/// divide that by 4096, comfortably inside the kernel's σ ≤ 2 reach. A
/// smaller ceiling would leave σ in the hundreds capped at the kernel's
/// radius and silently under-blurred.
const MAX_LEVELS: u32 = 12;

/// Symmetric taps one separable half-pass may use, the first being the
/// centre: radius 6 as three linear-sampled pairs.
const MAX_TAPS: usize = 4;

/// The uniform block every pass reads. Laid out for WGSL's uniform rules:
/// 16-byte alignment for the tap array, and a size that is a multiple of 16.
#[derive(Debug, Clone, Copy, Default)]
struct Params {
    texel: [f32; 2],
    axis: [f32; 2],
    ratio: f32,
    taps: u32,
    tap: [[f32; 2]; MAX_TAPS],
}

/// `Params`' WGSL size: two `vec2f`, `f32` + `u32` + `vec2f` padding, then
/// four `vec4f`.
const PARAMS_SIZE: u64 = 96;

impl Params {
    /// A same-size pass over a source of `width` × `height`.
    fn plain(width: u32, height: u32) -> Self {
        Self {
            texel: [1.0 / width as f32, 1.0 / height as f32],
            axis: [0.0, 0.0],
            ratio: 1.0,
            taps: 1,
            tap: [[0.0, 1.0], [0.0; 2], [0.0; 2], [0.0; 2]],
        }
    }

    fn with_ratio(mut self, ratio: f32) -> Self {
        self.ratio = ratio;
        self
    }

    /// One separable half-pass: `vertical` picks the axis, `kernel` the taps.
    fn with_kernel(mut self, kernel: &Kernel, vertical: bool) -> Self {
        self.axis = if vertical {
            [0.0, self.texel[1]]
        } else {
            [self.texel[0], 0.0]
        };
        self.taps = kernel.count;
        self.tap = kernel.tap;
        self
    }

    fn bytes(&self) -> [u8; PARAMS_SIZE as usize] {
        fn put(out: &mut [u8; PARAMS_SIZE as usize], offset: usize, value: f32) {
            out[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut out = [0_u8; PARAMS_SIZE as usize];
        put(&mut out, 0, self.texel[0]);
        put(&mut out, 4, self.texel[1]);
        put(&mut out, 8, self.axis[0]);
        put(&mut out, 12, self.axis[1]);
        put(&mut out, 16, self.ratio);
        out[20..24].copy_from_slice(&self.taps.to_ne_bytes());
        for (index, [offset, weight]) in self.tap.iter().enumerate() {
            put(&mut out, 32 + index * 16, *offset);
            put(&mut out, 36 + index * 16, *weight);
        }
        out
    }
}

/// One separable gaussian kernel as linear-sampled symmetric taps.
#[derive(Debug, Clone, Copy)]
struct Kernel {
    count: u32,
    /// `(offset in source texels, weight)`, entry 0 the centre. Normalized
    /// so `tap[0].1 + 2 · Σ tap[1..].1 == 1`.
    tap: [[f32; 2]; MAX_TAPS],
}

/// The kernel for one σ, in source texels.
///
/// Each non-centre tap replaces two adjacent texels with one bilinear fetch
/// at their weight-weighted midpoint, which is exact for a linear filter: a
/// bilinear sample at `i + t` returns `(1 − t)·s[i] + t·s[i+1]`, so placing
/// it at `t = w[i+1] / (w[i] + w[i+1])` and weighting it `w[i] + w[i+1]`
/// reproduces both texels' contributions.
///
/// The radius is capped at 6 (three pairs). σ ≤ 2 after decimation keeps
/// `ceil(3σ) ≤ 6`, so the cap only binds for a bake too small to decimate
/// far enough — a sliver the viewport intersection cut down — where it
/// under-blurs rather than misbehaving.
fn kernel(sigma: f64) -> Kernel {
    let mut tap = [[0.0_f32; 2]; MAX_TAPS];
    if !(sigma.is_finite() && sigma > 0.0) {
        tap[0] = [0.0, 1.0];
        return Kernel { count: 1, tap };
    }
    // Clamped to 0..=6 immediately: three linear-sampled pairs.
    let radius = ((3.0 * sigma).ceil() as u32).clamp(0, 2 * (MAX_TAPS as u32 - 1)) as usize;
    let mut weights = [0.0_f64; 13];
    let denominator = 2.0 * sigma * sigma;
    for (index, weight) in weights.iter_mut().enumerate().take(radius + 1) {
        let offset = index as f64;
        *weight = (-(offset * offset) / denominator).exp();
    }
    let mut total = weights[0];
    let mut count = 1_u32;
    let mut pairs = [[0.0_f64; 2]; MAX_TAPS];
    let mut index = 1_usize;
    while index <= radius {
        let (first, second) = (
            weights[index],
            weights.get(index + 1).copied().unwrap_or(0.0),
        );
        let combined = first + second;
        if combined <= 0.0 {
            break;
        }
        let offset = (first * index as f64 + second * (index + 1) as f64) / combined;
        pairs[count as usize] = [offset, combined];
        total += 2.0 * combined;
        count += 1;
        index += 2;
    }
    tap[0] = [0.0, (weights[0] / total) as f32];
    for slot in 1..count as usize {
        tap[slot] = [pairs[slot][0] as f32, (pairs[slot][1] / total) as f32];
    }
    Kernel { count, tap }
}

/// How many 2× decimation levels one bake takes, and the σ left at the
/// deepest one.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Decimation {
    levels: u32,
    sigma: f64,
}

/// Plans the decimation for `sigma` over a `width` × `height` bake.
///
/// Stops early once a level would leave either axis under two texels, where a
/// bilinear tap has nothing left to interpolate between.
fn decimate(sigma: f64, width: u32, height: u32) -> Decimation {
    let mut variance = sigma * sigma;
    let mut levels = 0;
    let (mut w, mut h) = (width, height);
    while variance > DECIMATE_ABOVE && levels < MAX_LEVELS {
        let next = (variance - RESAMPLE_VARIANCE) / 4.0;
        let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
        if next <= 0.0 || nw < 2 || nh < 2 {
            break;
        }
        variance = next;
        levels += 1;
        (w, h) = (nw, nh);
    }
    Decimation {
        levels,
        sigma: variance.max(0.0).sqrt(),
    }
}

/// Level `level`'s size, halving with `ceil` so no source column is dropped.
fn level_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    let (mut w, mut h) = (width.max(1), height.max(1));
    for _ in 0..level {
        (w, h) = (w.div_ceil(2), h.div_ceil(2));
    }
    (w, h)
}

/// The three fragment stages, so a pass names one without borrowing the
/// pipeline set that holds it.
#[derive(Debug, Clone, Copy)]
enum Stage {
    Premultiply,
    Resample,
    Gaussian,
}

/// One texture plus the view every pass binds it through.
struct Plane {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Plane {
    fn new(
        device: &wgpu::Device,
        label: &str,
        width: u32,
        height: u32,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            width: width.max(1),
            height: height.max(1),
            texture,
            view,
        }
    }
}

/// Every texture one filter group's bake needs, and the handle its result is
/// drawn through.
///
/// Three planes are always full-res — `target`, `levels[0]` and `output` —
/// and the rest depends on how far the bake decimates:
///
/// - **No decimation** (`sigma_device` <= 2): those three and nothing else. `pong` stays `None`,
///   because the horizontal half-pass writes into `target`, which the premultiply pass has already
///   consumed.
/// - **`n` levels**: plus `levels[1..=n]`, whose areas are a geometric quarter-series summing to
///   under a third of full-res, plus one `pong` at level `n`'s size.
///
/// So an admitted group costs between three and roughly four full-res RGBA8
/// textures of its own area, which is what [`MAX_FILTER_AREA`] is sized
/// against.
struct Bank {
    width: u32,
    height: u32,
    /// vello's own render target. Also the horizontal half-pass's
    /// destination when the bake takes no decimation level, which is sound
    /// because the premultiply pass has already consumed it by then — and is
    /// why an undecimated bake allocates no `pong`.
    target: Plane,
    /// The decimation pyramid, index 0 being the premultiplied full-res
    /// level. Grown to whatever depth a bake asks for and kept.
    levels: Vec<Plane>,
    /// The second half of the separable pair, at the deepest level's size.
    /// `None` until a decimated bake needs one, and reallocated when that
    /// level moves.
    pong: Option<Plane>,
    output: Plane,
    handle: ImageData,
}

/// Usage flags, named once so a texture's role is readable at its allocation.
const BAKE_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::STORAGE_BINDING
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::RENDER_ATTACHMENT);
const LEVEL_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TEXTURE_BINDING);
const OUTPUT_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC);

impl Bank {
    fn new(renderer: &mut vello::Renderer, device: &wgpu::Device, width: u32, height: u32) -> Self {
        let target = Plane::new(device, "dom filter bake", width, height, BAKE_USAGE);
        let output = Plane::new(device, "dom filter output", width, height, OUTPUT_USAGE);
        // `Renderer::register_texture` builds this handle too, but hardcodes
        // unpremultiplied alpha; the blur chain works premultiplied, so the
        // handle is built here with the alpha type that matches. The blob is
        // the same deliberate fake: vello never reads it, because the image
        // is registered as an override.
        let handle = ImageData {
            data: Blob::new(std::sync::Arc::new([])),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::AlphaPremultiplied,
            width: output.width,
            height: output.height,
        };
        renderer.override_image(
            &handle,
            Some(wgpu::TexelCopyTextureInfoBase {
                texture: output.texture.clone(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            }),
        );
        Self {
            width: output.width,
            height: output.height,
            target,
            levels: Vec::new(),
            pong: None,
            output,
            handle,
        }
    }

    /// Ensures the pyramid reaches `deepest`, allocating what is missing.
    fn ensure_levels(&mut self, device: &wgpu::Device, deepest: u32) {
        let wanted = deepest as usize + 1;
        while self.levels.len() < wanted {
            let level = self.levels.len() as u32;
            let (w, h) = level_size(self.width, self.height, level);
            self.levels
                .push(Plane::new(device, "dom filter level", w, h, LEVEL_USAGE));
        }
    }

    /// Ensures the separable pair's second plane exists at `width` × `height`.
    fn ensure_pong(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self
            .pong
            .as_ref()
            .is_some_and(|plane| plane.width == width && plane.height == height)
        {
            return;
        }
        self.pong = Some(Plane::new(
            device,
            "dom filter pong",
            width,
            height,
            LEVEL_USAGE,
        ));
    }
}

/// The pipelines, bind-group layout and sampler the passes share, built once
/// per [`FilterTextures`] on the first frame that blurs.
struct Pipelines {
    layout: wgpu::BindGroupLayout,
    premultiply: wgpu::RenderPipeline,
    resample: wgpu::RenderPipeline,
    gaussian: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl Pipelines {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dom filter blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dom filter blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(PARAMS_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dom filter blur"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let build = |label: &str, entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let premultiply = build("dom filter premultiply", "fs_premultiply");
        let resample = build("dom filter resample", "fs_resample");
        let gaussian = build("dom filter gaussian", "fs_gaussian");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("dom filter blur"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        Self {
            layout,
            premultiply,
            resample,
            gaussian,
            sampler,
        }
    }

    fn pipeline(&self, stage: Stage) -> &wgpu::RenderPipeline {
        match stage {
            Stage::Premultiply => &self.premultiply,
            Stage::Resample => &self.resample,
            Stage::Gaussian => &self.gaussian,
        }
    }
}

/// One group's chain of passes, recorded into one encoder and submitted once.
///
/// Every pass needs its own uniform block, because a queue write lands before
/// any command of the submission that follows it — one buffer rewritten
/// between passes would give every pass the last pass's parameters. The
/// buffers are pooled on [`FilterTextures`], so a settled page allocates none.
struct Passes<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    pipelines: &'a Pipelines,
    uniforms: &'a mut Vec<wgpu::Buffer>,
    next: usize,
    encoder: wgpu::CommandEncoder,
}

impl Passes<'_> {
    fn run(
        &mut self,
        stage: Stage,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        params: &Params,
    ) {
        if self.next == self.uniforms.len() {
            self.uniforms
                .push(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("dom filter params"),
                    size: PARAMS_SIZE,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        let uniform = &self.uniforms[self.next];
        self.next += 1;
        self.queue.write_buffer(uniform, 0, &params.bytes());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dom filter pass"),
            layout: &self.pipelines.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.pipelines.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dom filter pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(self.pipelines.pipeline(stage));
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn submit(self) {
        self.queue.submit([self.encoder.finish()]);
    }
}

/// The device side of `filter: blur()`: one bake texture set per filter group
/// of the frame being rendered, cached across frames. See the module doc.
#[derive(Default)]
pub struct FilterTextures {
    pipelines: Option<Pipelines>,
    /// One entry per filter group of the cached frame, `None` for a group
    /// over the budget.
    banks: Vec<Option<Bank>>,
    uniforms: Vec<wgpu::Buffer>,
    /// The scene one group's ops replay into. Reset per group, so its
    /// encoding capacity is reused across groups and frames.
    bake: vello::Scene,
    /// Index-parallel with the frame's filter groups: the handle the compose
    /// program draws each one through, or `None` for the raw fallback.
    images: Vec<Option<ImageData>>,
    /// Whether each group fits the budget, in program order. A field so the
    /// admission pass allocates nothing.
    admitted: Vec<bool>,
    /// The groups in bake order — increasing `ops.end`, so a nested group's
    /// texture exists before the group around it bakes.
    order: Vec<u32>,
    key: Option<(u64, u64)>,
}

impl std::fmt::Debug for FilterTextures {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilterTextures")
            .field("groups", &self.banks.len())
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl FilterTextures {
    /// Brings the bake textures up to `frame` and answers the table the
    /// compose program indexes by filter group.
    ///
    /// Cheap and allocation-free when the key has not moved, which is every
    /// frame that neither commits nor scrolls a blurred scroller's content.
    ///
    /// # Errors
    ///
    /// [`GpuError::Render`] if a bake render fails, in which case the table
    /// is emptied: every group falls back to replaying raw.
    #[expect(
        clippy::too_many_arguments,
        reason = "one bake pre-step's full inputs: the renderer's three device handles, the \
                  residency it shares, and the frame with its two compose inputs"
    )]
    pub fn prepare(
        &mut self,
        renderer: &mut vello::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut AtlasResidency,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
    ) -> Result<&[Option<ImageData>], GpuError> {
        let groups = frame.filter_groups();
        // A group whose content rides an inner scroll chain bakes different
        // pixels at a different offset; every other group moves *with* its
        // content, so its bake outlives any number of scroll frames.
        let generation = if groups.iter().any(|group| group.inner_chains) {
            scroll_generation
        } else {
            0
        };
        let key = (frame.commit_id(), generation);
        // The length check is a net, not the contract: a key carries no
        // document identity (see the module doc), and a table of the wrong
        // length is the one such mix-up that is cheap to catch.
        if self.key == Some(key) && self.images.len() == groups.len() {
            return Ok(&self.images);
        }
        // Dropped first: a failed bake must not leave a stale texture bound
        // to a group of a frame that is no longer the cached one.
        self.key = None;
        while self.banks.len() > groups.len() {
            if let Some(bank) = self.banks.pop().flatten() {
                renderer.override_image(&bank.handle, None);
            }
        }
        self.banks.resize_with(groups.len(), || None);
        self.images.clear();
        self.images.resize(groups.len(), None);
        if groups.is_empty() {
            self.key = Some(key);
            return Ok(&self.images);
        }
        if self.pipelines.is_none() {
            self.pipelines = Some(Pipelines::new(device));
        }
        self.plan(groups);
        self.bake_all(renderer, device, queue, atlas, frame, images, offset_of)?;
        self.key = Some(key);
        Ok(&self.images)
    }

    /// The table [`Self::prepare`] last produced — what a composite render
    /// has to name so the atlas residency stays truthful about it.
    #[must_use]
    pub fn images(&self) -> &[Option<ImageData>] {
        &self.images
    }

    /// Forgets which frame the bakes belong to, so the next
    /// [`Self::prepare`] re-bakes.
    ///
    /// For a target that changes documents: commit ids restart at one per
    /// document, so a retained key could otherwise match a different
    /// document's first frame. The textures stay allocated, since the next
    /// page's groups will want textures of their own.
    pub fn forget(&mut self) {
        self.key = None;
    }

    /// Decides which groups fit the budget, and the order to bake them in.
    fn plan(&mut self, groups: &[crate::FilterGroup]) {
        let mut budget = MAX_FILTER_AREA;
        self.admitted.clear();
        for group in groups {
            let (width, height) = group.size();
            let area = u64::from(width) * u64::from(height);
            let fits = width > 0
                && height > 0
                && width <= MAX_FILTER_DIMENSION
                && height <= MAX_FILTER_DIMENSION
                && area <= budget;
            if fits {
                budget -= area;
            }
            self.admitted.push(fits);
        }
        self.order.clear();
        self.order.extend(
            (0..groups.len())
                .map(|index| u32::try_from(index).expect("a frame cannot hold 2^32 filters")),
        );
        // Post-order: a group's `ops` range contains every nested group's, so
        // ordering by range end alone puts the inner ones first.
        self.order
            .sort_unstable_by_key(|&index| groups[index as usize].ops.end);
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one bake pre-step's full inputs; see `prepare`"
    )]
    fn bake_all(
        &mut self,
        renderer: &mut vello::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut AtlasResidency,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
    ) -> Result<(), GpuError> {
        let Self {
            pipelines,
            banks,
            uniforms,
            bake,
            images: filtered,
            admitted,
            order,
            ..
        } = self;
        let pipelines = pipelines
            .as_ref()
            .expect("the pipelines are built before the first bake");
        let groups = frame.filter_groups();
        for &index in order.iter() {
            let index = index as usize;
            if !admitted[index] {
                continue;
            }
            let group = &groups[index];
            let (width, height) = group.size();
            let bank = ensure_bank(&mut banks[index], renderer, device, width, height);

            bake.reset();
            frame.bake_filter(index, bake, images, filtered, offset_of);
            // Every render through this renderer owes the residency a pass,
            // including a bake: a patch-free bake frees the whole image
            // atlas, which is exactly the loss `AtlasResidency` repairs.
            atlas.prepare_all(renderer, bake, images, filtered);
            renderer
                .render_to_texture(
                    device,
                    queue,
                    bake,
                    &bank.target.view,
                    &render_params(vello::peniko::Color::TRANSPARENT, width, height),
                )
                .map_err(|error| GpuError::Render(error.to_string()))?;

            let plan = decimate(f64::from(group.sigma), width, height);
            bank.ensure_levels(device, plan.levels);
            // Only a decimated bake needs a second plane for the separable
            // pair; at level 0 the bake target is free again and serves. See
            // `run_chain`.
            if plan.levels > 0 {
                let (deep_w, deep_h) = level_size(width, height, plan.levels);
                bank.ensure_pong(device, deep_w, deep_h);
            }

            let mut passes = Passes {
                device,
                queue,
                pipelines,
                uniforms,
                next: 0,
                encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("dom filter blur"),
                }),
            };
            run_chain(&mut passes, bank, &plan);
            passes.submit();

            // The texture's contents just changed, so the copy vello holds in
            // its atlas is stale whatever the residency believes about it.
            renderer.mark_override_image_dirty(&bank.handle);
            filtered[index] = Some(bank.handle.clone());
        }
        Ok(())
    }
}

/// Records one group's whole pass chain against its bank.
fn run_chain(passes: &mut Passes<'_>, bank: &Bank, plan: &Decimation) {
    let levels = &bank.levels;
    let full = &levels[0];
    passes.run(
        Stage::Premultiply,
        &bank.target.view,
        &full.view,
        &Params::plain(bank.width, bank.height),
    );
    for level in 1..=plan.levels as usize {
        let source = &levels[level - 1];
        passes.run(
            Stage::Resample,
            &source.view,
            &levels[level].view,
            &Params::plain(source.width, source.height).with_ratio(2.0),
        );
    }
    let deepest = &levels[plan.levels as usize];
    let kernel = kernel(plan.sigma);
    // The horizontal half-pass needs somewhere to put its result that is not
    // its own source. A decimated bake uses `pong`, allocated at the deepest
    // level's size; an undecimated one uses the *bake target*, which the
    // premultiply pass has already read and which is full-res by
    // construction — so no full-res plane is allocated for it.
    let (pong, vertical_target) = if plan.levels == 0 {
        (&bank.target, &bank.output)
    } else {
        let pong = bank
            .pong
            .as_ref()
            .expect("a decimated bake ensures its pong before running the chain");
        debug_assert!(
            pong.width == deepest.width && pong.height == deepest.height,
            "the pong is ensured at the deepest level's size",
        );
        (pong, deepest)
    };
    passes.run(
        Stage::Gaussian,
        &deepest.view,
        &pong.view,
        &Params::plain(deepest.width, deepest.height).with_kernel(&kernel, false),
    );
    passes.run(
        Stage::Gaussian,
        &pong.view,
        &vertical_target.view,
        &Params::plain(pong.width, pong.height).with_kernel(&kernel, true),
    );
    for level in (1..=plan.levels as usize).rev() {
        let source = &levels[level];
        let target = if level == 1 {
            &bank.output
        } else {
            &levels[level - 1]
        };
        passes.run(
            Stage::Resample,
            &source.view,
            &target.view,
            &Params::plain(source.width, source.height).with_ratio(0.5),
        );
    }
}

/// The bank for one group at `width` × `height`, reusing the one already
/// there when it is the right size.
///
/// A resize is a new texture and therefore a new handle: vello keys an
/// override by blob id and assumes the override's dimensions match the
/// image's, so the old handle is unregistered rather than repointed.
fn ensure_bank<'bank>(
    slot: &'bank mut Option<Bank>,
    renderer: &mut vello::Renderer,
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> &'bank mut Bank {
    if !slot
        .as_ref()
        .is_some_and(|bank| bank.width == width && bank.height == height)
    {
        if let Some(stale) = slot.take() {
            renderer.override_image(&stale.handle, None);
        }
        *slot = Some(Bank::new(renderer, device, width, height));
    }
    slot.as_mut().expect("the bank was just ensured")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        DECIMATE_ABOVE, MAX_FILTER_DIMENSION, MAX_LEVELS, MAX_TAPS, Params, decimate, kernel,
        level_size,
    };

    /// A kernel is a probability distribution: the centre plus twice every
    /// symmetric tap has to be one, or the blur changes the image's total
    /// brightness.
    #[test]
    fn every_kernel_sums_to_one() {
        for sigma in [0.1_f64, 0.5, 1.0, 1.75, 2.0, 3.0, 8.0] {
            let kernel = kernel(sigma);
            assert!(
                kernel.count >= 1 && kernel.count as usize <= MAX_TAPS,
                "sigma {sigma} asked for {} taps",
                kernel.count,
            );
            let total: f32 = kernel.tap[0][1]
                + 2.0
                    * kernel.tap[1..kernel.count as usize]
                        .iter()
                        .map(|tap| tap[1])
                        .sum::<f32>();
            assert!(
                (total - 1.0).abs() < 1e-5,
                "sigma {sigma} summed to {total}"
            );
        }
    }

    /// Each pair tap sits between the two texels it stands in for, nearer the
    /// heavier one — which for a gaussian is always the inner texel.
    #[test]
    fn pair_taps_sit_between_their_two_texels() {
        let kernel = kernel(2.0);
        for (index, tap) in kernel.tap[1..kernel.count as usize].iter().enumerate() {
            let first = (2 * index + 1) as f32;
            assert!(
                tap[0] > first && tap[0] < first + 1.0,
                "tap {index} at {} left the [{first}, {}] span",
                tap[0],
                first + 1.0,
            );
        }
    }

    /// Decimation runs until the residual sigma is inside the kernel's reach,
    /// and each level accounts for the resample pair's own variance.
    #[test]
    fn decimation_stops_at_a_kernel_sized_sigma() {
        for sigma in [1.0_f64, 2.0, 4.0, 16.0, 64.0, 400.0, 1365.0] {
            let plan = decimate(sigma, 4096, 4096);
            assert!(
                plan.sigma * plan.sigma <= DECIMATE_ABOVE + 1e-9,
                "sigma {sigma} left {} at level {}",
                plan.sigma,
                plan.levels,
            );
            let reconstructed = (0..plan.levels).fold(plan.sigma * plan.sigma, |variance, _| {
                variance * 4.0 + super::RESAMPLE_VARIANCE
            });
            assert!(
                (reconstructed.sqrt() - sigma).abs() < 1e-6,
                "sigma {sigma} does not reconstruct from level {}",
                plan.levels,
            );
        }
        assert_eq!(
            decimate(1.0, 64, 64).levels,
            0,
            "a small sigma decimates none"
        );
    }

    /// The largest sigma a bake rect can carry — the one whose own 6 sigma
    /// extent fills `MAX_FILTER_DIMENSION` — still decimates into the
    /// kernel's reach rather than stopping at `MAX_LEVELS` and under-blurring.
    #[test]
    fn the_largest_bakeable_sigma_still_reaches_the_kernel() {
        let plan = decimate(1365.0, MAX_FILTER_DIMENSION, MAX_FILTER_DIMENSION);
        assert!(
            plan.levels < MAX_LEVELS,
            "the size floor, not the level ceiling, is what stops this bake \
             (stopped at {})",
            plan.levels,
        );
        assert!(
            plan.sigma <= 2.0,
            "sigma 1365 must decimate to within the kernel's reach, got {}",
            plan.sigma,
        );
    }

    /// A bake too narrow to halve stops decimating rather than producing a
    /// one-texel level a bilinear tap has nothing to interpolate across.
    ///
    /// Only a bake the viewport intersection cut into a sliver can get here:
    /// a rect carries a 3 sigma margin on both sides, so an uncut one is at
    /// least 6 sigma across and always decimates far enough.
    #[test]
    fn a_sliver_stops_decimating_before_it_vanishes() {
        // 3 -> 2 is legal, 2 -> 1 is not.
        assert_eq!(decimate(400.0, 3, 4096).levels, 1);
        // 5 -> 3 -> 2, then stop.
        assert_eq!(decimate(400.0, 5, 4096).levels, 2);
        assert_eq!(decimate(400.0, 2, 4096).levels, 0);
        // And the residual sigma is capped to what the kernel covers, so the
        // pass is well formed even when the sliver stopped it early.
        let plan = decimate(400.0, 2, 4096);
        assert!(super::kernel(plan.sigma).count as usize <= MAX_TAPS);
    }

    /// Levels halve with `ceil`, so no source column is ever dropped.
    #[test]
    fn level_sizes_round_up() {
        assert_eq!(level_size(7, 3, 1), (4, 2));
        assert_eq!(level_size(7, 3, 2), (2, 1));
        assert_eq!(level_size(7, 3, 0), (7, 3));
    }

    /// The uniform block's WGSL offsets, which nothing but this test can see.
    #[test]
    fn the_uniform_block_lands_where_wgsl_reads_it() {
        let params = Params::plain(4, 8).with_ratio(2.0);
        let bytes = params.bytes();
        let at = |offset: usize| f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert!((at(0) - 0.25).abs() < 1e-7, "texel.x");
        assert!((at(4) - 0.125).abs() < 1e-7, "texel.y");
        assert!((at(16) - 2.0).abs() < 1e-7, "ratio");
        assert_eq!(
            u32::from_ne_bytes(bytes[20..24].try_into().unwrap()),
            1,
            "taps",
        );
        assert!((at(36) - 1.0).abs() < 1e-7, "the centre tap's weight");
    }
}
