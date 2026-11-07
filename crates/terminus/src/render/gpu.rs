//! The wgpu side: instance, adapter and device creation, and rendering built
//! scenes, including a headless render-to-texture path with pixel readback
//! (the test and screenshot surface; the painter's window stack is
//! `crate::painter::graphics`).
//!
//! [`instance`] and [`GpuDevice::request`] are the one place this crate
//! creates wgpu objects: the headless target here and the painter's window
//! both build their device through them, so both run the rasterizer on the
//! same features and limits.
//!
//! Every target owns one [`Raster`], and beside it a
//! [`super::vector_textures::VectorTextures`] and a
//! [`super::blur::FilterTextures`]. Those two are optional per frame: a frame
//! drawing no SVG document asks nothing of the first, and one with no
//! `filter: blur()` group and no `backdrop-filter` element nothing of the
//! second. Where a frame owes both, the vector textures come first: a filter
//! bake replays ops that may draw them.

use std::fmt;

use euclid::default::Vector2D;

use super::blur::FilterTextures;
use super::raster::Raster;
use super::vector_textures::VectorTextures;
use crate::paint::display_list::DisplayList;
use crate::peniko::ImageData;
use crate::visual::{CommittedFrame, ScrollSlot};

/// Headless GPU renderer for tests, benchmarks, and windowless embedders.
pub struct Headless {
    gpu: GpuDevice,
    raster: Raster,
    filters: FilterTextures,
    vectors: VectorTextures,
    target: Option<RenderTarget>,
    readback: Option<ReadbackBuffer>,
}

/// A wgpu instance over the backends and flags the environment selects
/// (`WGPU_BACKEND` and wgpu's other variables), every compiled-in backend
/// otherwise.
pub(crate) fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        display: None,
        backends: wgpu::Backends::from_env().unwrap_or_default(),
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
    })
}

/// One adapter's device and queue, which a target renders through.
pub(crate) struct GpuDevice {
    adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
}

impl GpuDevice {
    /// The limits every device is created with: wgpu's defaults, which wgpu
    /// guarantees on every modern backend and in WebGPU.
    /// [`crate::render::image::MAX_RENDERABLE_DIMENSION`] is their
    /// `max_texture_dimension_2d`.
    pub(crate) const LIMITS: wgpu::Limits = wgpu::Limits::defaults();

    /// Requests a device on the adapter the environment selects
    /// (`WGPU_ADAPTER_NAME`, else the default one), able to present to
    /// `compatible_surface` if one is given. No optional feature is
    /// requested: the rasterizer and the bakes need none.
    ///
    /// `None` when no adapter qualifies or it refuses the device.
    pub(crate) async fn request(
        instance: &wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Option<Self> {
        let adapter =
            wgpu::util::initialize_adapter_from_env_or_default(instance, compatible_surface)
                .await
                .ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("terminus"),
                required_features: wgpu::Features::empty(),
                required_limits: Self::LIMITS,
                ..wgpu::DeviceDescriptor::default()
            })
            .await
            .ok()?;
        Some(Self {
            adapter,
            device,
            queue,
        })
    }

    /// The adapter the device was created on.
    pub(crate) fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
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

impl Headless {
    /// Creates a headless renderer on the platform's default adapter.
    ///
    /// Returns [`GpuError::NoAdapter`] when the platform has no usable adapter
    /// rather than panicking: an embedder can surface that and fall back, while
    /// tests treat it as a hard environment failure.
    pub fn new() -> Result<Self, GpuError> {
        let gpu =
            pollster::block_on(GpuDevice::request(&instance(), None)).ok_or(GpuError::NoAdapter)?;
        let raster = Raster::new(&gpu.device);
        Ok(Self {
            gpu,
            raster,
            filters: FilterTextures::default(),
            vectors: VectorTextures::default(),
            target: None,
            readback: None,
        })
    }

    /// Drops the render target this renderer retained, and the readback
    /// buffer sized for it.
    ///
    /// For a target that changes documents. The target is given up rather than
    /// kept because it *is* the last frame here — there is no surface in front
    /// of it, so a reader would otherwise be handed the previous document's
    /// pixels as this one's. [`Self::render_frame`] builds a new one, and
    /// [`Self::read_pixels`] has nothing to read until it does.
    ///
    /// The filter bakes keep their textures but forget which frame they were
    /// baked for, since commit ids restart at one per document. The vector
    /// textures are untouched: their key is a process-unique image identity,
    /// which no document change can alias.
    pub fn forget(&mut self) {
        self.target = None;
        self.readback = None;
        self.filters.forget();
    }

    /// Forgets which frame the offscreen bakes belong to, keeping the
    /// render target.
    ///
    /// Every consumer that points this renderer at a *second document* owes
    /// this call: the bake cache is keyed by commit id, and commit ids
    /// restart at one per document. [`Self::forget`] includes it.
    pub fn forget_filters(&mut self) {
        self.filters.forget();
    }

    /// Bakes the textures of `frame`'s vector image draws that are not
    /// already resident, if it has any, and answers the table
    /// [`crate::visual::CommittedFrame::compose_into`] takes for them.
    ///
    /// Call before [`Self::prepare_filters`] and before composing: a filter
    /// bake replays ops that may draw these textures. A frame with no
    /// vector draw answers an empty slice and touches no GPU resource.
    ///
    /// # Errors
    ///
    /// [`GpuError::Render`] if a bake render fails.
    pub fn prepare_vectors(
        &mut self,
        frame: &CommittedFrame,
    ) -> Result<&[Option<ImageData>], GpuError> {
        let Self {
            gpu,
            raster,
            vectors,
            ..
        } = self;
        vectors.prepare(frame, raster, &gpu.device, &gpu.queue)
    }

    /// Bakes `frame`'s `filter: blur()` groups and `backdrop-filter`
    /// elements, if it has any, and answers the table
    /// [`crate::visual::CommittedFrame::compose_into`] takes.
    ///
    /// Call before composing, at the same `animation_now` the composition
    /// will use, with the `vectors` table [`Self::prepare_vectors`] answered
    /// for this frame, and with the `base_color` [`Self::render_frame`] will
    /// be given for it: a `backdrop-filter` element at the document's root
    /// filters the canvas along with what the page painted over it. A frame
    /// with no filter entry answers an empty slice and touches no GPU
    /// resource.
    ///
    /// # Errors
    ///
    /// [`GpuError::Render`] if a bake render fails.
    #[expect(
        clippy::too_many_arguments,
        reason = "one bake pre-step's full inputs: the frame, its two pixel tables, the three \
                  compose readings, and the canvas colour"
    )]
    pub fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        vectors: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
        base_color: crate::peniko::Color,
    ) -> Result<&[Option<ImageData>], GpuError> {
        let Self {
            gpu,
            raster,
            filters,
            ..
        } = self;
        filters.prepare(
            raster,
            &gpu.device,
            &gpu.queue,
            frame,
            images,
            vectors,
            offset_of,
            scroll_generation,
            animation_now,
            base_color,
        )
    }

    /// Replaces the vector raster cache with one bounded by `bytes`, so a
    /// test can watch eviction without drawing 64 MiB of pictures.
    #[cfg(test)]
    pub(crate) fn set_vector_budget(&mut self, bytes: u64) {
        self.vectors = VectorTextures::with_budget(bytes);
    }

    /// The vector raster cache, for a test's assertions on residency.
    #[cfg(test)]
    pub(crate) fn vector_textures(&self) -> &VectorTextures {
        &self.vectors
    }

    /// Renders a scene into the retained headless texture.
    pub fn render_frame(
        &mut self,
        scene: &DisplayList,
        width: u32,
        height: u32,
        base_color: crate::peniko::Color,
    ) -> Result<(), GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::Render(format!(
                "render target must be non-empty, got {width}\u{d7}{height}"
            )));
        }

        self.ensure_target(width, height);
        let Self {
            gpu,
            raster,
            target,
            ..
        } = self;
        let view = &target
            .as_ref()
            .expect("ensure_target installs a render target")
            .view;
        if let Err(error) = raster.render_to_texture(
            &gpu.device,
            &gpu.queue,
            scene,
            view,
            width,
            height,
            base_color,
        ) {
            *target = None;
            return Err(error);
        }
        Ok(())
    }

    /// Waits for all submitted GPU work.
    pub fn wait_idle(&self) -> Result<(), GpuError> {
        self.gpu
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
            gpu,
            target,
            readback,
            ..
        } = self;
        let device = &gpu.device;
        let queue = &gpu.queue;
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
                })
                .and_then(|()| {
                    slice
                        .get_mapped_range()
                        .map_err(|error| GpuError::Render(error.to_string()))
                });
            match waited {
                Ok(mapped) => {
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
        scene: &DisplayList,
        width: u32,
        height: u32,
        base_color: crate::peniko::Color,
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

        let device = &self.gpu.device;
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
            usage: Raster::TARGET_USAGE.union(wgpu::TextureUsages::COPY_SRC),
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

        let device = &self.gpu.device;
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
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_texture(
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

    let mapped = slice
        .get_mapped_range()
        .map_err(|error| GpuError::Render(error.to_string()))?;
    let mut pixels = Vec::with_capacity(tight_bytes_per_row as usize * height as usize);
    for row in mapped.chunks_exact(padded_bytes_per_row as usize) {
        pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
    }
    Ok(pixels)
}
