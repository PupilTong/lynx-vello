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
//!   platform has no usable adapter — embedders can surface that error, while tests treat it as a
//!   hard environment failure.
//! - `Headless::render_frame`: render into a retained `Rgba8Unorm` storage texture through
//!   `Renderer::render_to_texture`, with no CPU synchronization.
//! - `Headless::read_pixels`: copy the last target into a retained padded readback buffer (256-byte
//!   row alignment), block on the map (`map_async` + an indefinite device poll), and return
//!   tightly-packed RGBA8 rows. `Headless::render` composes the two for screenshot callers.
//! - `Headless::wait_idle` bounds in-flight work for paced frame loops; [`read_texture`] is the
//!   one-shot readback for embedders that render on their own device; [`renderer_options`] and
//!   [`render_params`] are the single render policy every pulsar target must construct with.

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

/// Renderer construction options for pulsar's one render policy: area-only
/// antialiasing. Windowed embedders building their own [`vello::Renderer`]
/// must construct it with these options to match the headless path.
#[must_use]
pub fn renderer_options() -> vello::RendererOptions {
    vello::RendererOptions {
        antialiasing_support: vello::AaSupport::area_only(),
        ..vello::RendererOptions::default()
    }
}

/// Per-frame render parameters for pulsar's render policy over `base_color`.
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
        })
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

        let error = {
            let slice = readback.buffer.slice(..);
            let (sender, receiver) = mpsc::channel();
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
                    let mapped = slice
                        .get_mapped_range()
                        .map_err(|error| GpuError::Render(error.to_string()))?;
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
        label: Some("pulsar texture readback"),
        size: u64::from(padded_bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pulsar texture readback copy"),
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
    let (sender, receiver) = mpsc::channel();
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

    let mapped = slice
        .get_mapped_range()
        .map_err(|error| GpuError::Render(error.to_string()))?;
    let mut pixels = Vec::with_capacity(tight_bytes_per_row as usize * height as usize);
    for row in mapped.chunks_exact(padded_bytes_per_row as usize) {
        pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
    }
    Ok(pixels)
}
