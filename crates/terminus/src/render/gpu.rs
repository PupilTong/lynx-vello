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
use crate::paint::display_list::{DisplayList, Texture};
use crate::peniko::ImageData;
use crate::visual::{CommittedFrame, ScrollSlot};

/// Headless GPU renderer for tests, benchmarks, and windowless embedders.
///
/// Its device and renderer are the process's [`SharedGpu`]; the render
/// target, the readback buffer, the bake caches and its [`Raster`] are its
/// own.
pub struct Headless {
    gpu: Arc<SharedGpu>,
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

/// One adapter's device and queue, and the rasterizer state every target
/// on them shares. See the [module documentation](self).
pub(crate) struct SharedGpu {
    /// Kept so a window's surface is created on the instance its adapter
    /// is checked against.
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    /// Written to by any target; submitted to only through
    /// [`Self::submit`].
    pub(crate) queue: wgpu::Queue,
    pub(crate) raster: SharedRaster,
    /// Held across every submit and every surface configuration; see
    /// [`Self::configure`].
    submits: Mutex<()>,
}

impl fmt::Debug for SharedGpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedGpu")
            .field("adapter", &self.adapter.get_info().name)
            .finish_non_exhaustive()
    }
}

/// The process's live [`SharedGpu`], held weakly so it goes with the last
/// target that uses it.
#[cfg(not(target_arch = "wasm32"))]
static SHARED_GPU: Mutex<Weak<SharedGpu>> = Mutex::new(Weak::new());

// A browser build keeps one per thread: a WebGPU device belongs to the
// Worker that requested it, and wgpu's handles are not `Send` there.
#[cfg(target_arch = "wasm32")]
thread_local! {
    static SHARED_GPU: std::cell::RefCell<Weak<SharedGpu>> =
        const { std::cell::RefCell::new(Weak::new()) };
}

/// Runs `f` on the live-stack slot, locked.
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

/// The stack `slot` holds if it is live, else one `make` builds, which
/// `slot` then holds.
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
    /// The limits every device is created with: wgpu's defaults, which wgpu
    /// guarantees on every modern backend and in WebGPU.
    /// [`crate::render::image::MAX_RENDERABLE_DIMENSION`] is their
    /// `max_texture_dimension_2d`.
    pub(crate) const LIMITS: wgpu::Limits = wgpu::Limits::defaults();

    /// The live shared stack, or a new one on the adapter the environment
    /// selects if none is live. Blocks on the adapter and device requests.
    ///
    /// # Errors
    ///
    /// [`GpuError::NoAdapter`] when no adapter qualifies or it refuses the
    /// device.
    pub(crate) fn offscreen() -> Result<Arc<Self>, GpuError> {
        // The slot stays locked through the build, so two targets built at
        // once get one stack rather than racing to two.
        with_shared_slot(|slot| {
            upgrade_or_build(slot, || {
                pollster::block_on(Self::build(instance(), None)).ok_or(GpuError::NoAdapter)
            })
        })
    }

    /// A surface for `target` and the stack it is drawn with: the live
    /// shared stack when its adapter can present to the surface, otherwise
    /// a stack on an adapter that can.
    ///
    /// The second case is a machine with several adapters where the shared
    /// one cannot present to this window; that window then has a device and
    /// a renderer of its own. A stack built when none is live becomes the
    /// shared one.
    ///
    /// # Errors
    ///
    /// The surface's creation error, or the message that no adapter can
    /// present to it.
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

    /// The stack a `surface` created on `instance` is drawn with: `live`
    /// when its adapter can present to it, else a new one. Not locked across
    /// the device request: a browser build has no other thread to contend
    /// with, and a native race at worst leaves one window on a stack of its
    /// own.
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

    /// Requests a device on the adapter the environment selects
    /// (`WGPU_ADAPTER_NAME`, else the default one), able to present to
    /// `compatible_surface` if one is given, and builds the rasterizer on
    /// it. No optional feature is requested: the rasterizer and the bakes
    /// need none.
    ///
    /// `None` when no adapter qualifies or it refuses the device.
    pub(crate) async fn build(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Option<Self> {
        Self::build_with(instance, compatible_surface, SharedRaster::new).await
    }

    /// [`Self::build`], with `raster` building the rasterizer on the device.
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
            submits: Mutex::new(()),
        })
    }

    /// Submits `buffers` to the queue. Every submit of every target goes
    /// through here.
    pub(crate) fn submit(
        &self,
        buffers: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> wgpu::SubmissionIndex {
        let _held = self.submits.lock().unwrap_or_else(PoisonError::into_inner);
        self.queue.submit(buffers)
    }

    /// Configures `surface`, which the stack's adapter presents to, with
    /// `config`.
    ///
    /// wgpu waits for the device to go idle before it configures a surface,
    /// and fails the configuration if the queue is not empty once the wait
    /// returns — which a submit by another target's thread in the meantime
    /// makes it. So no submit runs during a configuration: both hold the
    /// same lock.
    pub(crate) fn configure(
        &self,
        surface: &wgpu::Surface<'_>,
        config: &wgpu::SurfaceConfiguration,
    ) {
        let _held = self.submits.lock().unwrap_or_else(PoisonError::into_inner);
        surface.configure(&self.device, config);
    }

    /// A stack no other target shares, its rasterizer built by `raster`:
    /// for a test that watches the shared state.
    #[cfg(test)]
    pub(crate) fn private(
        raster: impl FnOnce(&wgpu::Device, &wgpu::Queue) -> SharedRaster,
    ) -> Arc<Self> {
        Arc::new(
            pollster::block_on(Self::build_with(instance(), None, raster))
                .expect("a usable GPU adapter is mandatory"),
        )
    }

    /// The adapter the device was created on.
    pub(crate) fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// The instance the adapter was enumerated on, which a surface it is
    /// checked against must be created on.
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
    /// Creates a headless renderer on the process's [`SharedGpu`], building
    /// it on the platform's default adapter if no target holds one.
    ///
    /// Returns [`GpuError::NoAdapter`] when the platform has no usable adapter
    /// rather than panicking: an embedder can surface that and fall back, while
    /// tests treat it as a hard environment failure.
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
    ) -> Result<&[Option<Texture>], GpuError> {
        let Self {
            gpu,
            raster,
            vectors,
            ..
        } = self;
        vectors.prepare(frame, raster, gpu)
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

    /// The stack this target draws through.
    #[cfg(test)]
    pub(crate) fn gpu(&self) -> &Arc<SharedGpu> {
        &self.gpu
    }

    /// A target on `gpu` rather than on the process's stack.
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

    /// Waits for all submitted GPU work, every target's on the shared
    /// device included.
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

    /// A target built while another is live draws through that one's device
    /// and renderer rather than building its own.
    #[test]
    fn a_target_built_while_another_is_live_shares_its_stack() {
        let first = Headless::new().expect("a usable GPU adapter is mandatory");
        let second = Headless::new().expect("a usable GPU adapter is mandatory");
        assert!(Arc::ptr_eq(first.gpu(), second.gpu()));
    }

    /// The slot hands out the live stack while a holder keeps it, builds
    /// none then, and holds nothing once the last holder is gone.
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

    /// Targets hold their stack and nothing else does: once the last of
    /// them has rendered and gone, so has the stack.
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
