//! The wgpu side: adapter/device management and rendering built scenes,
//! including a headless render-to-texture path with pixel readback (the
//! test and screenshot surface — embedders with a window drive
//! `vello::util::RenderContext`/`RenderSurface` themselves through the
//! [`crate::vello`] re-export).
//!
//! Spec sketch (agent: gpu):
//! - `Headless::new()`: `vello::util::RenderContext::new()`, pick a device via
//!   `context.device(None)` (pollster-blocked); construct one `vello::Renderer` with area-AA
//!   support only (the one AA mode pulsar renders with). Return `Err(NoAdapter)` cleanly when the
//!   platform has no usable adapter — callers (tests) skip rather than fail.
//! - `Headless::render_frame`: render into a retained `Rgba8Unorm` storage texture through
//!   `Renderer::render_to_texture`, with no CPU synchronization.
//! - `Headless::read_pixels`: copy the last target into a retained padded readback buffer (256-byte
//!   row alignment), block on the map (`map_async` + an indefinite device poll), and return
//!   tightly-packed RGBA8 rows. `Headless::render` composes the two for screenshot callers.

use std::fmt;
use std::sync::mpsc;

use vello::util::RenderContext;
use vello::wgpu;

/// Headless GPU renderer for tests, benchmarks, and windowless embedders.
pub struct Headless {
    context: RenderContext,
    device_index: usize,
    renderer: vello::Renderer,
    target: Option<RenderTarget>,
    readback: Option<ReadbackBuffer>,
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

/// Why a [`Headless`] renderer could not be created or render.
#[derive(Debug)]
pub enum GpuError {
    /// No usable GPU adapter on this machine (headless CI, forbidden GPU
    /// access): callers should skip GPU work, not fail.
    NoAdapter,
    /// Device/queue creation or rendering failed.
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

impl Headless {
    /// Creates a headless renderer on the platform's default adapter.
    pub fn new() -> Result<Self, GpuError> {
        let mut context = RenderContext::new();
        let device_index = pollster::block_on(context.device(None)).ok_or(GpuError::NoAdapter)?;
        let handle = &context.devices[device_index];
        let renderer = vello::Renderer::new(
            &handle.device,
            vello::RendererOptions {
                antialiasing_support: vello::AaSupport::area_only(),
                ..vello::RendererOptions::default()
            },
        )
        .map_err(|error| GpuError::Render(error.to_string()))?;
        Ok(Self {
            context,
            device_index,
            renderer,
            target: None,
            readback: None,
        })
    }

    /// Renders `scene` at `width` × `height` device px over `base_color`.
    ///
    /// The render target is retained and reused while its dimensions stay the
    /// same. This is the continuous-frame path for windowless embedders:
    /// unlike [`Self::render`], it does not synchronize the CPU with the GPU
    /// for pixel readback.
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
        let target = target
            .as_ref()
            .expect("ensure_target installs a render target");
        renderer
            .render_to_texture(
                &handle.device,
                &handle.queue,
                scene,
                &target.view,
                &vello::RenderParams {
                    base_color,
                    width,
                    height,
                    antialiasing_method: vello::AaConfig::Area,
                },
            )
            .map_err(|error| GpuError::Render(error.to_string()))?;
        Ok(())
    }

    /// Reads the most recently rendered frame as tightly-packed, row-major
    /// RGBA8 pixels.
    ///
    /// The staging buffer is retained across captures of the same size.
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

        // wgpu requires texture→buffer copy rows padded to 256 bytes; copy
        // padded, then strip the padding while assembling the result.
        let tight_bytes_per_row = width * 4;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pulsar headless readback copy"),
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

        let slice = readback.buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        // Waiting for the queue to drain resolves the map and fires the
        // callback, so the recv below never blocks.
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| GpuError::Render(error.to_string()))?;
        receiver
            .recv()
            .map_err(|_| GpuError::Render("readback map callback dropped".to_owned()))?
            .map_err(|error| GpuError::Render(error.to_string()))?;

        let mapped = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity(tight_bytes_per_row as usize * height as usize);
        for row in mapped.chunks_exact(readback.padded_bytes_per_row as usize) {
            pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
        }
        drop(mapped);
        readback.buffer.unmap();
        Ok(pixels)
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
            label: Some("pulsar headless target"),
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
            label: Some("pulsar headless readback"),
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
