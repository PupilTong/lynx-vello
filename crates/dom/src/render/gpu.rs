//! The wgpu side: adapter/device management and rendering built scenes,
//! including a headless render-to-texture path with pixel readback (the
//! test and screenshot surface — embedders with a window drive
//! `vello::util::RenderContext`/`RenderSurface` themselves through the
//! [`crate::vello`] re-export).
//!
//! There is one render policy in this crate: area-only antialiasing.
//! [`renderer_options`] and [`render_params`] are its single definition, and
//! every target rendered through this crate — the headless one here and an
//! embedder's windowed one — must be constructed from them, or a windowed
//! frame will not match a headless screenshot of the same scene.

use std::fmt;

use vello::util::RenderContext;
use vello::wgpu;

/// Headless GPU renderer for tests, benchmarks, and windowless embedders.
pub struct Headless {
    context: RenderContext,
    device_index: usize,
    renderer: vello::Renderer,
    target: Option<RenderTarget>,
    readback: Option<ReadbackBuffer>,
    planes: PlaneBank,
}

/// The retained plane textures of one layered frame, registered with one
/// renderer as drawable images.
///
/// A commit re-bakes each of the frame's planes once; every frame after
/// that composes them as textured draws. Each composite render re-copies
/// the plane textures into vello's image atlas (one GPU texture-to-texture
/// copy per plane, window-sized): vello 0.9 frees its persistent atlas
/// whenever a scene without images renders — the bake scenes here included
/// — while its cache still counts the planes resident and clean, so pixels
/// only survive under an every-use dirty mark. The scroll frame still
/// encodes and rasterizes none of the scroller content.
#[derive(Default)]
pub struct PlaneBank {
    /// The commit the retained textures were baked from.
    commit: Option<(u64, u64)>,
    planes: Vec<Plane>,
    images: Vec<vello::peniko::ImageData>,
    bake_scene: vello::Scene,
}

struct Plane {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl fmt::Debug for PlaneBank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaneBank")
            .field("commit", &self.commit)
            .field("planes", &self.planes.len())
            .finish_non_exhaustive()
    }
}

impl PlaneBank {
    /// Brings the retained textures up to `frame`'s plan: on a new commit,
    /// each plane is (re)baked into its texture; textures are reused across
    /// commits while their sizes hold. Call once before every composite
    /// render — every call re-marks the planes dirty so the atlas re-copy
    /// happens on use (see the type docs for why that is mandatory).
    ///
    /// # Panics
    ///
    /// If `frame` has no composite plan.
    pub fn prepare(
        &mut self,
        renderer: &mut vello::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &crate::visual::CommittedFrame,
        pixels: &dyn crate::FrameImages,
        image_epoch: u64,
    ) -> Result<(), GpuError> {
        let plan = frame
            .composite_plan()
            .expect("prepare bakes a layered frame's plan");
        // Keyed on the image epoch as well as the commit: a plane baked
        // before its images loaded must re-bake when they arrive, or the
        // image is permanently absent from that scroller.
        if self.commit == Some((frame.commit_id(), image_epoch)) {
            for image in &self.images {
                renderer.mark_override_image_dirty(image);
            }
            return Ok(());
        }
        while self.planes.len() > plan.plane_count() {
            self.planes.pop();
            renderer.unregister_texture(self.images.pop().expect("images pair with planes"));
        }
        for index in 0..plan.plane_count() {
            let (width, height) = plan.plane_size(index);
            let sized = self.planes.get(index).is_some_and(|plane| {
                plane.texture.width() == width && plane.texture.height() == height
            });
            if !sized {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("dom retained plane"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let image = renderer.register_texture(texture.clone());
                if index < self.planes.len() {
                    self.planes[index] = Plane { texture, view };
                    renderer.unregister_texture(std::mem::replace(&mut self.images[index], image));
                } else {
                    self.planes.push(Plane { texture, view });
                    self.images.push(image);
                }
            }
            self.bake_scene.reset();
            frame.bake_plane(index, &mut self.bake_scene, pixels);
            renderer
                .render_to_texture(
                    device,
                    queue,
                    &self.bake_scene,
                    &self.planes[index].view,
                    &render_params(vello::peniko::Color::TRANSPARENT, width, height),
                )
                .map_err(|error| GpuError::Render(error.to_string()))?;
            renderer.mark_override_image_dirty(&self.images[index]);
        }
        self.commit = Some((frame.commit_id(), image_epoch));
        Ok(())
    }

    /// The registered images, index-parallel with the plan's planes — what
    /// [`crate::visual::CommittedFrame::composite_into`] draws.
    #[must_use]
    pub fn images(&self) -> &[vello::peniko::ImageData] {
        &self.images
    }
}

#[derive(Debug)]
struct RenderTarget {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

#[derive(Debug)]
struct ReadbackBuffer {
    padded_bytes_per_row: u32,
    height: u32,
    buffer: wgpu::Buffer,
}

impl fmt::Debug for Headless {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Headless").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum GpuError {
    NoAdapter,
    Render(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter => f.write_str("no usable GPU adapter"),
            Self::Render(message) => write!(f, "GPU rendering failed: {message}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Renderer construction options for this crate's one render policy:
/// area-only antialiasing. Windowed embedders building their own
/// [`vello::Renderer`] must construct it with these options to match the
/// headless path.
#[must_use]
pub fn renderer_options() -> vello::RendererOptions {
    vello::RendererOptions {
        antialiasing_support: vello::AaSupport::area_only(),
        ..vello::RendererOptions::default()
    }
}

/// Per-frame render parameters for that same policy over `base_color`.
#[must_use]
pub fn render_params(
    base_color: vello::peniko::Color,
    width: u32,
    height: u32,
) -> vello::RenderParams {
    vello::RenderParams {
        base_color,
        width,
        height,
        antialiasing_method: vello::AaConfig::Area,
    }
}

impl Headless {
    /// Creates a headless renderer on the platform's default adapter.
    ///
    /// Returns [`GpuError::NoAdapter`] when the platform has no usable adapter
    /// rather than panicking: an embedder can surface that and fall back, while
    /// tests treat it as a hard environment failure.
    pub fn new() -> Result<Self, GpuError> {
        let mut context = RenderContext::new();
        let device_index = pollster::block_on(context.device(None)).ok_or(GpuError::NoAdapter)?;
        let handle = &context.devices[device_index];
        let renderer = vello::Renderer::new(&handle.device, renderer_options())
            .map_err(|error| GpuError::Render(error.to_string()))?;
        Ok(Self {
            context,
            device_index,
            renderer,
            target: None,
            readback: None,
            planes: PlaneBank::default(),
        })
    }

    /// Brings this renderer's retained plane textures up to `frame`'s plan;
    /// see [`PlaneBank::prepare`].
    ///
    /// # Panics
    ///
    /// If `frame` has no composite plan.
    pub fn prepare_planes(
        &mut self,
        frame: &crate::visual::CommittedFrame,
        pixels: &dyn crate::FrameImages,
        image_epoch: u64,
    ) -> Result<(), GpuError> {
        let Self {
            context,
            device_index,
            renderer,
            planes,
            ..
        } = self;
        let handle = &context.devices[*device_index];
        planes.prepare(
            renderer,
            &handle.device,
            &handle.queue,
            frame,
            pixels,
            image_epoch,
        )
    }

    /// The retained planes' registered images; see [`PlaneBank::images`].
    #[must_use]
    pub fn plane_images(&self) -> &[vello::peniko::ImageData] {
        self.planes.images()
    }

    /// Renders a scene into the retained headless texture.
    pub fn render_frame(
        &mut self,
        scene: &vello::Scene,
        width: u32,
        height: u32,
        base_color: vello::peniko::Color,
    ) -> Result<(), GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::Render(format!(
                "render target must be non-empty, got {width}\u{d7}{height}"
            )));
        }

        self.ensure_target(width, height);
        let Self {
            context,
            device_index,
            renderer,
            target,
            ..
        } = self;
        let handle = &context.devices[*device_index];
        let view = &target
            .as_ref()
            .expect("ensure_target installs a render target")
            .view;
        if let Err(error) = renderer.render_to_texture(
            &handle.device,
            &handle.queue,
            scene,
            view,
            &render_params(base_color, width, height),
        ) {
            *target = None;
            return Err(GpuError::Render(error.to_string()));
        }
        Ok(())
    }

    /// Waits for all submitted GPU work.
    pub fn wait_idle(&self) -> Result<(), GpuError> {
        let handle = &self.context.devices[self.device_index];
        handle
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| GpuError::Render(error.to_string()))?;
        Ok(())
    }

    /// Reads the last frame as tightly packed row-major RGBA8 pixels.
    pub fn read_pixels(&mut self) -> Result<Vec<u8>, GpuError> {
        let (width, height) = self
            .target
            .as_ref()
            .map(|target| (target.width, target.height))
            .ok_or_else(|| GpuError::Render("no headless frame has been rendered".to_owned()))?;
        self.ensure_readback(width, height);

        let Self {
            context,
            device_index,
            target,
            readback,
            ..
        } = self;
        let handle = &context.devices[*device_index];
        let device = &handle.device;
        let queue = &handle.queue;
        let target = target
            .as_ref()
            .expect("a target was checked before allocating readback");
        let readback = readback
            .as_ref()
            .expect("ensure_readback installs a staging buffer");

        let tight_bytes_per_row = width * 4;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("dom headless readback copy"),
        });
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(readback.padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let error = {
            let slice = readback.buffer.slice(..);
            let (sender, receiver) = flume::bounded(1);
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
            let waited = device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|error| GpuError::Render(error.to_string()))
                .and_then(|_| {
                    receiver
                        .recv()
                        .map_err(|_| GpuError::Render("readback map callback dropped".to_owned()))?
                        .map_err(|error| GpuError::Render(error.to_string()))
                });
            match waited {
                Ok(()) => {
                    let mapped = slice.get_mapped_range();
                    let mut pixels =
                        Vec::with_capacity(tight_bytes_per_row as usize * height as usize);
                    for row in mapped.chunks_exact(readback.padded_bytes_per_row as usize) {
                        pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
                    }
                    drop(mapped);
                    readback.buffer.unmap();
                    return Ok(pixels);
                }
                Err(error) => error,
            }
        };
        self.readback = None;
        Err(error)
    }

    /// Renders a frame and reads it back as tightly-packed RGBA8 pixels.
    pub fn render(
        &mut self,
        scene: &vello::Scene,
        width: u32,
        height: u32,
        base_color: vello::peniko::Color,
    ) -> Result<Vec<u8>, GpuError> {
        self.render_frame(scene, width, height, base_color)?;
        self.read_pixels()
    }

    fn ensure_target(&mut self, width: u32, height: u32) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.width == width && target.height == height)
        {
            return;
        }

        let device = &self.context.devices[self.device_index].device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dom headless target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target = Some(RenderTarget {
            width,
            height,
            texture,
            view,
        });
    }

    fn ensure_readback(&mut self, width: u32, height: u32) {
        let padded_bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        if self.readback.as_ref().is_some_and(|readback| {
            readback.padded_bytes_per_row == padded_bytes_per_row && readback.height == height
        }) {
            return;
        }

        let device = &self.context.devices[self.device_index].device;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dom headless readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.readback = Some(ReadbackBuffer {
            padded_bytes_per_row,
            height,
            buffer,
        });
    }
}

/// Reads an RGBA8 texture into tightly packed row-major pixels.
pub fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, GpuError> {
    if width == 0 || height == 0 {
        return Err(GpuError::Render(format!(
            "readback source must be non-empty, got {width}\u{d7}{height}"
        )));
    }

    let tight_bytes_per_row = width * 4;
    let padded_bytes_per_row =
        tight_bytes_per_row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dom texture readback"),
        size: u64::from(padded_bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("dom texture readback copy"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    let (sender, receiver) = flume::bounded(1);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| GpuError::Render(error.to_string()))?;
    receiver
        .recv()
        .map_err(|_| GpuError::Render("readback map callback dropped".to_owned()))?
        .map_err(|error| GpuError::Render(error.to_string()))?;

    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity(tight_bytes_per_row as usize * height as usize);
    for row in mapped.chunks_exact(padded_bytes_per_row as usize) {
        pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
    }
    Ok(pixels)
}
