//! The wgpu side: instance, adapter and device creation, and rendering built
//! scenes, including a headless render-to-texture path with pixel readback
//! (the test and screenshot surface; the painter's window stack is
//! `crate::painter::graphics`).
//!
//! # One stack per process
//!
//! Every target in a process — each [`Headless`] and each painter window —
//! draws through one [`SharedGpu`]: one wgpu device and queue, and on them
//! one `vello_gpu` renderer with its resources and the bitmaps uploaded to
//! them ([`super::sparse::SharedRaster`]). The shaders are compiled once, and
//! a bitmap two views draw is uploaded once. The stack is held weakly
//! ([`SharedGpu::offscreen`], [`SharedGpu::for_surface`]) and goes with the
//! last target that holds it; a native one is process-wide, a wasm32 one per
//! thread, since a WebGPU device belongs to the Worker that requested it. A
//! window whose surface the shared adapter cannot present to gets a stack of
//! its own.
//!
//! [`SharedGpu::build`] is the one place this crate creates a device, so a
//! window and a headless target run the rasterizer on the same features and
//! limits.
//!
//! Every target owns one [`Raster`] on the stack — its render state: the
//! scene it fills, the textures it binds, and its pins on the shared
//! bitmaps — and beside it a
//! [`super::vector_textures::VectorTextures`] and a
//! [`super::blur::FilterTextures`]. Those two are optional per frame: a frame
//! drawing no SVG document asks nothing of the first, and one with no
//! `filter: blur()` group and no `backdrop-filter` element nothing of the
//! second. Where a frame owes both, the vector textures come first: a filter
//! bake replays ops that may draw them.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use euclid::default::Vector2D;

use super::blur::FilterTextures;
use super::raster::Raster;
use super::sparse::SharedRaster;
use super::vector_textures::VectorTextures;
use crate::paint::display_list::{DisplayList, ImageRef, Texture};
use crate::visual::{CommittedFrame, ScrollSlot};

pub struct Headless {
    gpu: Arc<SharedGpu>,
    raster: Raster,
    filters: FilterTextures,
    vectors: VectorTextures,
    target: Option<RenderTarget>,
    readback: Option<ReadbackBuffer>,
}

pub(crate) fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        display: None,
        backends: wgpu::Backends::from_env().unwrap_or_default(),
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
    })
}

pub(crate) struct SharedGpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) raster: SharedRaster,
    pub(super) canvases: super::canvas::Textures,
    submits: Mutex<()>,
}

impl fmt::Debug for SharedGpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedGpu")
            .field("adapter", &self.adapter.get_info().name)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
static SHARED_GPU: Mutex<Weak<SharedGpu>> = Mutex::new(Weak::new());

#[cfg(target_arch = "wasm32")]
thread_local! {
    static SHARED_GPU: std::cell::RefCell<Weak<SharedGpu>> =
        const { std::cell::RefCell::new(Weak::new()) };
}

fn with_shared_slot<R>(f: impl FnOnce(&mut Weak<SharedGpu>) -> R) -> R {
    #[cfg(not(target_arch = "wasm32"))]
    {
        f(&mut SHARED_GPU.lock().unwrap_or_else(PoisonError::into_inner))
    }
    #[cfg(target_arch = "wasm32")]
    {
        SHARED_GPU.with(|slot| f(&mut slot.borrow_mut()))
    }
}

pub(crate) fn live() -> Option<Arc<SharedGpu>> {
    with_shared_slot(|slot| slot.upgrade())
}

#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::arc_with_non_send_sync,
        reason = "a wasm32 stack stays on the thread that built it (its slot is thread-local); \
                  `Arc` keeps one type for both targets"
    )
)]
fn upgrade_or_build(
    slot: &mut Weak<SharedGpu>,
    make: impl FnOnce() -> Result<SharedGpu, GpuError>,
) -> Result<Arc<SharedGpu>, GpuError> {
    if let Some(live) = slot.upgrade() {
        return Ok(live);
    }
    let stack = Arc::new(make()?);
    *slot = Arc::downgrade(&stack);
    Ok(stack)
}

impl SharedGpu {
    pub(crate) const LIMITS: wgpu::Limits = wgpu::Limits::defaults();

    pub(crate) fn offscreen() -> Result<Arc<Self>, GpuError> {
        with_shared_slot(|slot| {
            upgrade_or_build(slot, || {
                pollster::block_on(Self::build(instance(), None)).ok_or(GpuError::NoAdapter)
            })
        })
    }

    pub(crate) async fn for_surface(
        target: wgpu::SurfaceTarget<'static>,
    ) -> Result<(Arc<Self>, wgpu::Surface<'static>), String> {
        let live = with_shared_slot(|slot| slot.upgrade());
        let instance = live
            .as_ref()
            .map_or_else(instance, |gpu| gpu.instance.clone());
        let surface = instance
            .create_surface(target)
            .map_err(|error| error.to_string())?;
        let gpu = Self::presenting(live, instance, &surface).await?;
        Ok((gpu, surface))
    }

    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::arc_with_non_send_sync,
            reason = "a wasm32 stack stays on the thread that built it (its slot is thread-local); \
                      `Arc` keeps one type for both targets"
        )
    )]
    pub(crate) async fn presenting(
        live: Option<Arc<Self>>,
        instance: wgpu::Instance,
        surface: &wgpu::Surface<'_>,
    ) -> Result<Arc<Self>, String> {
        if let Some(live) = live
            && live.adapter.is_surface_supported(surface)
        {
            return Ok(live);
        }
        let built = Arc::new(
            Self::build(instance, Some(surface))
                .await
                .ok_or_else(|| "no GPU device can present to this window".to_owned())?,
        );
        with_shared_slot(|slot| {
            if slot.strong_count() == 0 {
                *slot = Arc::downgrade(&built);
            }
        });
        Ok(built)
    }

    pub(crate) async fn build(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Option<Self> {
        Self::build_with(instance, compatible_surface, SharedRaster::new).await
    }

    async fn build_with(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
        raster: impl FnOnce(&wgpu::Device, &wgpu::Queue) -> SharedRaster,
    ) -> Option<Self> {
        let adapter =
            wgpu::util::initialize_adapter_from_env_or_default(&instance, compatible_surface)
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
        let raster = raster(&device, &queue);
        Some(Self {
            instance,
            adapter,
            device,
            queue,
            raster,
            canvases: super::canvas::Textures::default(),
            submits: Mutex::new(()),
        })
    }

    pub(crate) fn submit(
        &self,
        buffers: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> wgpu::SubmissionIndex {
        let _held = self.submits.lock().unwrap_or_else(PoisonError::into_inner);
        self.queue.submit(buffers)
    }

    pub(crate) fn configure(
        &self,
        surface: &wgpu::Surface<'_>,
        config: &wgpu::SurfaceConfiguration,
    ) {
        let _held = self.submits.lock().unwrap_or_else(PoisonError::into_inner);
        surface.configure(&self.device, config);
    }

    #[cfg(test)]
    pub(crate) fn private(
        raster: impl FnOnce(&wgpu::Device, &wgpu::Queue) -> SharedRaster,
    ) -> Arc<Self> {
        Arc::new(
            pollster::block_on(Self::build_with(instance(), None, raster))
                .expect("a usable GPU adapter is mandatory"),
        )
    }

    pub(crate) fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    #[cfg_attr(
        not(all(test, target_os = "macos")),
        expect(dead_code, reason = "read by the window stack's test alone")
    )]
    pub(crate) fn instance(&self) -> &wgpu::Instance {
        &self.instance
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
    pub fn new() -> Result<Self, GpuError> {
        let gpu = SharedGpu::offscreen()?;
        let raster = Raster::new(&gpu);
        Ok(Self {
            gpu,
            raster,
            filters: FilterTextures::default(),
            vectors: VectorTextures::default(),
            target: None,
            readback: None,
        })
    }

    pub fn forget(&mut self) {
        self.target = None;
        self.readback = None;
        self.filters.forget();
    }

    pub fn forget_filters(&mut self) {
        self.filters.forget();
    }

    pub fn prepare_vectors(
        &mut self,
        frame: &CommittedFrame,
    ) -> Result<&[Option<Texture>], GpuError> {
        let Self {
            gpu,
            raster,
            vectors,
            ..
        } = self;
        vectors.prepare(frame, raster, gpu)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one bake pre-step's full inputs: the frame, its two pixel tables, the three \
                  compose readings, and the canvas colour"
    )]
    pub fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageRef>],
        vectors: &[Option<Texture>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
        base_color: crate::peniko::Color,
    ) -> Result<&[Option<Texture>], GpuError> {
        let Self {
            gpu,
            raster,
            filters,
            ..
        } = self;
        filters.prepare(
            raster,
            gpu,
            frame,
            images,
            vectors,
            offset_of,
            scroll_generation,
            animation_now,
            base_color,
        )
    }

    pub(crate) fn gpu(&self) -> &Arc<SharedGpu> {
        &self.gpu
    }

    pub(crate) fn lost_pixels(&self) -> u64 {
        self.raster.lost_pixels()
    }

    #[cfg(test)]
    pub(crate) fn on(gpu: Arc<SharedGpu>) -> Self {
        Self {
            raster: Raster::new(&gpu),
            gpu,
            filters: FilterTextures::default(),
            vectors: VectorTextures::default(),
            target: None,
            readback: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_vector_budget(&mut self, bytes: u64) {
        self.vectors = VectorTextures::with_budget(bytes);
    }

    #[cfg(test)]
    pub(crate) fn vector_textures(&self) -> &VectorTextures {
        &self.vectors
    }

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
        let Self { raster, target, .. } = self;
        let view = &target
            .as_ref()
            .expect("ensure_target installs a render target")
            .view;
        if let Err(error) = raster.render_to_texture(scene, view, width, height, base_color) {
            *target = None;
            return Err(error);
        }
        Ok(())
    }

    pub fn wait_idle(&self) -> Result<(), GpuError> {
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| GpuError::Render(error.to_string()))?;
        Ok(())
    }

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
        gpu.submit([encoder.finish()]);

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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_texture(
    gpu: &SharedGpu,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, GpuError> {
    let device = &gpu.device;
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
    gpu.submit([encoder.finish()]);

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::{Arc, Weak};

    use super::{Headless, SharedGpu, instance, upgrade_or_build};
    use crate::kurbo::{Affine, Rect};
    use crate::paint::display_list::DisplayList;
    use crate::peniko::{Color, Fill};
    use crate::render::sparse::SharedRaster;

    #[test]
    fn a_target_built_while_another_is_live_shares_its_stack() {
        let first = Headless::new().expect("a usable GPU adapter is mandatory");
        let second = Headless::new().expect("a usable GPU adapter is mandatory");
        assert!(Arc::ptr_eq(first.gpu(), second.gpu()));
    }

    #[test]
    fn the_shared_slot_keeps_no_stack_alive() {
        let build = || {
            pollster::block_on(SharedGpu::build(instance(), None)).ok_or(super::GpuError::NoAdapter)
        };
        let mut slot = Weak::new();
        let first = upgrade_or_build(&mut slot, build).expect("a usable GPU adapter is mandatory");
        let second = upgrade_or_build(&mut slot, || unreachable!("a live stack is handed out"))
            .expect("the live stack");
        assert!(Arc::ptr_eq(&first, &second));
        drop((first, second));
        assert!(slot.upgrade().is_none(), "the slot kept the stack alive");
    }

    #[test]
    fn the_stack_goes_with_the_last_target() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let held = Arc::downgrade(&gpu);
        let mut targets = [Headless::on(Arc::clone(&gpu)), Headless::on(gpu)];
        let mut scene = DisplayList::new();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Rect::new(0.0, 0.0, 4.0, 4.0),
        );
        for target in &mut targets {
            target
                .render(&scene, 4, 4, Color::WHITE)
                .expect("the scene renders");
        }
        let [first, second] = targets;
        drop(first);
        assert!(held.upgrade().is_some(), "the second target holds it");
        drop(second);
        assert!(held.upgrade().is_none(), "the stack outlived its targets");
    }
}
