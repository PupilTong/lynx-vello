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
//! - `Headless::render(scene, width, height, base_color) -> Vec<u8>`: render into an `Rgba8Unorm`
//!   storage texture via `Renderer::render_to_texture`, copy to a padded readback buffer (256-byte
//!   row alignment), block on the map (`map_async` + an indefinite device poll), and return
//!   tightly-packed RGBA8 rows.

use std::fmt;
use std::sync::mpsc;

use vello::util::RenderContext;
use vello::wgpu;

/// Headless GPU renderer for tests, benchmarks, and windowless embedders.
pub struct Headless {
    context: RenderContext,
    device_index: usize,
    renderer: vello::Renderer,
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
        })
    }

    /// Renders `scene` at `width` × `height` device px over `base_color`
    /// and reads back tightly-packed RGBA8 pixels (row-major).
    pub fn render(
        &mut self,
        scene: &vello::Scene,
        width: u32,
        height: u32,
        base_color: vello::peniko::Color,
    ) -> Result<Vec<u8>, GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::Render(format!(
                "render target must be non-empty, got {width}\u{d7}{height}"
            )));
        }
        let handle = &self.context.devices[self.device_index];
        let device = &handle.device;
        let queue = &handle.queue;

        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pulsar headless target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.renderer
            .render_to_texture(
                device,
                queue,
                scene,
                &view,
                &vello::RenderParams {
                    base_color,
                    width,
                    height,
                    antialiasing_method: vello::AaConfig::Area,
                },
            )
            .map_err(|error| GpuError::Render(error.to_string()))?;

        // wgpu requires texture→buffer copy rows padded to 256 bytes; copy
        // padded, then strip the padding while assembling the result.
        let tight_bytes_per_row = width * 4;
        let padded_bytes_per_row =
            tight_bytes_per_row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pulsar headless readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pulsar headless readback copy"),
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);

        let slice = readback.slice(..);
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
        for row in mapped.chunks_exact(padded_bytes_per_row as usize) {
            pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
        }
        drop(mapped);
        readback.unmap();
        Ok(pixels)
    }
}
