//! The painter's window presentation stack.
//!
//! Built on the thread that owns the window, because creating a surface from
//! a window handle is a main-thread-only call on macOS — and that is the same
//! thread that draws, so it is never handed anywhere: every acquire, render
//! and present happens on the thread that built the painter. Rendering into the
//! retained
//! target texture and presenting it are separate steps: the target is
//! re-rendered only when a new scene was produced, while surface
//! invalidation and re-exposure re-present the retained target with a blit
//! alone.
//!
//! A frame is three calls in order: [`WindowGraphics::acquire`], then
//! [`WindowGraphics::render_to_target`] if a new scene was produced, then
//! [`WindowGraphics::present`]. Acquiring first is deliberate. On
//! `PresentMode::AutoVsync` the swap chain hands over an image only once one
//! is free, so `acquire` is where a frame's back-pressure lands, and every
//! swap-chain image in flight is another display refresh between the wait and
//! the scan-out. Doing the wait up front puts the whole frame — the clock
//! reading included — on the near side of that pipeline, so what is sampled
//! is the frame being produced for the next refresh, not one produced a
//! pipeline-depth earlier.
//!
//! The surface is created on the process's [`SharedGpu`] and drawn with its
//! renderer, so a window pays for vello's fixed working buffers only when no
//! other target in the process has already; the surface, its retained target
//! texture and the bake caches are the window's own.

use std::sync::Arc;

use euclid::default::Vector2D;
use vello::peniko::ImageData;
use vello::wgpu;

use super::{CANVAS, FrameSize, PainterError};
use crate::render::blur::FilterTextures;
#[cfg(not(target_arch = "wasm32"))]
use crate::render::gpu::read_texture;
use crate::render::gpu::{SharedGpu, SharedRenderer, render_params};
use crate::render::vector_textures::VectorTextures;
use crate::visual::{CommittedFrame, ScrollSlot};

/// The draw target an embedder lends a painter.
///
/// `'static` deliberately: the painter owns the surface built from it for the
/// rest of its life, which outlives any borrow the host could lend. A
/// windowing embedder passes a shared handle — an `Arc<winit::Window>` —
/// rather than a reference.
pub type WindowTarget = vello::wgpu::SurfaceTarget<'static>;

/// The swap-chain image a frame will be presented in, taken before the frame
/// is produced. Dropping it without [`WindowGraphics::present`] discards it,
/// which is what an abandoned frame does.
pub(super) struct AcquiredFrame {
    texture: vello::wgpu::SurfaceTexture,
    /// The surface handed over a suboptimal image and wants reconfiguring
    /// once this frame is on screen.
    reconfigure_after: bool,
}

/// What the surface had to give this frame.
pub(super) enum FrameAcquisition {
    Ready(AcquiredFrame),
    /// Nothing available — transiently, so the caller asks for another frame
    /// rather than treating it as a failure.
    Retry,
}

pub(super) struct WindowGraphics {
    /// The device and renderer this window draws with — the process's shared
    /// stack, unless its adapter could not present to this window.
    gpu: Arc<SharedGpu>,
    surface: WindowSurface,
    /// The `filter: blur()` bakes of the frame this window last composed;
    /// see [`crate::render::blur::FilterTextures`].
    filters: FilterTextures,
    /// The textures the vector images drawn in this window were baked into,
    /// kept across frames under a byte budget; see
    /// [`crate::render::vector_textures::VectorTextures`].
    vectors: VectorTextures,
    #[cfg(not(target_arch = "wasm32"))]
    capture: Option<CaptureTarget>,
}

/// A window's swap chain and the texture vello renders into before the blit
/// to it: what `vello::util::RenderSurface` holds, minus the index into a
/// context of devices this crate no longer keeps.
///
/// vello renders through a storage binding, which a surface texture usually
/// cannot be, hence the intermediate target.
struct WindowSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    target_view: wgpu::TextureView,
    blitter: wgpu::util::TextureBlitter,
}

impl std::fmt::Debug for WindowSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowSurface")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl WindowSurface {
    /// What `vello::util::RenderContext::create_render_surface` builds: the
    /// first 8-bit RGBA or BGRA format the surface offers, two frames of
    /// latency, and a target texture of the same size.
    fn new(
        gpu: &SharedGpu,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, PainterError> {
        let format = surface
            .get_capabilities(gpu.adapter())
            .formats
            .into_iter()
            .find(|format| {
                matches!(
                    format,
                    wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                )
            })
            .ok_or_else(|| {
                PainterError::Render("the window surface offers no 8-bit RGBA format".to_owned())
            })?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            color_space: wgpu::SurfaceColorSpace::Auto,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        let window = Self {
            surface,
            config,
            target_view: target_view(&gpu.device, width, height),
            blitter: wgpu::util::TextureBlitter::new(&gpu.device, format),
        };
        window.configure(gpu);
        Ok(window)
    }

    fn configure(&self, gpu: &SharedGpu) {
        self.surface.configure(&gpu.device, &self.config);
    }

    /// Reallocates the target at the new size and reconfigures the swap
    /// chain. Both sides must be non-zero.
    fn resize(&mut self, gpu: &SharedGpu, width: u32, height: u32) {
        self.target_view = target_view(&gpu.device, width, height);
        self.config.width = width;
        self.config.height = height;
        self.configure(gpu);
    }
}

/// The texture vello renders a window's frame into, which the present step
/// blits to the swap chain and a capture blits to its readback twin.
fn target_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("painter window target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            format: wgpu::TextureFormat::Rgba8Unorm,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// A `COPY_SRC` twin of the surface's render target, on the same device, so
/// screenshots read back exactly what the window pipeline rendered instead
/// of re-rendering on a second GPU stack.
#[cfg(not(target_arch = "wasm32"))]
struct CaptureTarget {
    width: u32,
    height: u32,
    texture: vello::wgpu::Texture,
    view: vello::wgpu::TextureView,
    blitter: vello::wgpu::util::TextureBlitter,
}

impl std::fmt::Debug for WindowGraphics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowGraphics")
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl WindowGraphics {
    pub(super) async fn new(
        target: impl Into<WindowTarget>,
        size: FrameSize,
    ) -> Result<Self, PainterError> {
        let (gpu, surface) = SharedGpu::for_surface(target.into())
            .await
            .map_err(|error| PainterError::Render(error.to_string()))?;
        let surface = WindowSurface::new(&gpu, surface, size.width.max(1), size.height.max(1))?;
        Ok(Self {
            gpu,
            surface,
            filters: FilterTextures::default(),
            vectors: VectorTextures::default(),
            #[cfg(not(target_arch = "wasm32"))]
            capture: None,
        })
    }

    /// Reconfigures the surface when the target size moved.
    ///
    /// What was rendered into the retained target is not discarded here: the
    /// painter's own record of it carries the size it was rendered for, so a
    /// resize already fails to match and the next frame is painted.
    fn configure_for(&mut self, size: FrameSize) {
        if size.width != 0
            && size.height != 0
            && (self.surface.config.width != size.width
                || self.surface.config.height != size.height)
        {
            self.surface.resize(&self.gpu, size.width, size.height);
        }
    }

    /// Takes the swap-chain image this frame will be presented in, before any
    /// of the frame's work is done.
    ///
    /// The surface is brought to `size` first, so the image handed back is the
    /// one the coming render actually fits. This call is the frame's vsync
    /// wait; a caller that gets [`FrameAcquisition::Retry`] has no image this
    /// frame and must ask for another.
    pub(super) fn acquire(&mut self, size: FrameSize) -> Result<FrameAcquisition, PainterError> {
        self.configure_for(size);
        let Self { gpu, surface, .. } = self;
        let (texture, reconfigure_after) = match surface.surface.get_current_texture() {
            vello::wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            vello::wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            vello::wgpu::CurrentSurfaceTexture::Timeout
            | vello::wgpu::CurrentSurfaceTexture::Occluded => return Ok(FrameAcquisition::Retry),
            vello::wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(gpu);
                return Ok(FrameAcquisition::Retry);
            }
            vello::wgpu::CurrentSurfaceTexture::Lost => {
                return Err(PainterError::Render(
                    "the window surface was lost".to_owned(),
                ));
            }
            vello::wgpu::CurrentSurfaceTexture::Validation => {
                return Err(PainterError::Render(
                    "surface acquisition raised a wgpu validation error".to_owned(),
                ));
            }
        };
        Ok(FrameAcquisition::Ready(AcquiredFrame {
            texture,
            reconfigure_after,
        }))
    }

    /// Renders one composed scene into the retained target texture, which
    /// [`Self::present`] then blits into a swap-chain image.
    ///
    /// `images` are the bitmaps the scene draws, which the atlas residency
    /// needs to keep the shared renderer's uploads truthful.
    pub(super) fn render_to_target(
        &mut self,
        scene: &vello::Scene,
        images: &[Option<ImageData>],
        size: FrameSize,
    ) -> Result<(), PainterError> {
        self.configure_for(size);
        let mut shared = self.gpu.renderer();
        let SharedRenderer { renderer, atlas } = &mut *shared;
        // The filter and vector bakes are override images this scene may
        // draw, so the residency has to see them too or a post-loss repair
        // is missed.
        atlas.prepare_all(
            renderer,
            scene,
            images,
            self.filters.images(),
            self.vectors.images(),
        );
        renderer
            .render_to_texture(
                &self.gpu.device,
                &self.gpu.queue,
                scene,
                &self.surface.target_view,
                &render_params(CANVAS, size.width, size.height),
            )
            .map_err(|error| PainterError::Render(error.to_string()))
    }

    /// Bakes the textures of `frame`'s vector image draws not already
    /// resident, on the window's device.
    ///
    /// Call before [`Self::prepare_filters`] and before composing; a frame
    /// with no vector draw asks nothing of the device.
    pub(super) fn prepare_vectors(
        &mut self,
        frame: &CommittedFrame,
    ) -> Result<&[Option<ImageData>], PainterError> {
        let Self { gpu, vectors, .. } = self;
        let mut shared = gpu.renderer();
        let SharedRenderer { renderer, atlas } = &mut *shared;
        vectors
            .prepare(frame, renderer, &gpu.device, &gpu.queue, atlas)
            .map_err(|error| PainterError::Gpu(error.to_string()))
    }

    /// Bakes `frame`'s `filter: blur()` groups and `backdrop-filter`
    /// elements on the window's device.
    ///
    /// Call before composing, with the `vectors` table
    /// [`Self::prepare_vectors`] answered for this frame; a frame with no
    /// filter entry asks nothing of the device.
    pub(super) fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        vectors: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
    ) -> Result<&[Option<ImageData>], PainterError> {
        let Self { gpu, filters, .. } = self;
        let mut shared = gpu.renderer();
        let SharedRenderer { renderer, atlas } = &mut *shared;
        filters
            .prepare(
                renderer,
                &gpu.device,
                &gpu.queue,
                atlas,
                frame,
                images,
                vectors,
                offset_of,
                scroll_generation,
                animation_now,
                CANVAS,
            )
            .map_err(|error| PainterError::Gpu(error.to_string()))
    }

    /// Forgets which frame the filter bakes belong to. The window keeps its
    /// surface and its last presented frame; only the bake cache's identity
    /// is given up, because commit ids restart per document.
    pub(super) fn forget_filters(&mut self) {
        self.filters.forget();
    }

    /// Presents the retained target into the image [`Self::acquire`] took:
    /// blits and presents. Pure composition — no document anywhere, nobody
    /// blocked behind this.
    pub(super) fn present(&mut self, acquired: AcquiredFrame) {
        let AcquiredFrame {
            texture: surface_texture,
            reconfigure_after,
        } = acquired;
        let Self { gpu, surface, .. } = self;
        let output_view = surface_texture
            .texture
            .create_view(&vello::wgpu::TextureViewDescriptor::default());
        let mut encoder =
            gpu.device
                .create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
                    label: Some("painter window blit"),
                });
        surface.blitter.copy(
            &gpu.device,
            &mut encoder,
            &surface.target_view,
            &output_view,
        );
        gpu.queue.submit([encoder.finish()]);
        gpu.queue.present(surface_texture);
        if reconfigure_after {
            surface.configure(gpu);
        }
    }

    /// Reads back the frame most recently rendered into the surface's target
    /// texture, on the window's device, as tightly-packed RGBA8 pixels.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn capture_frame(&mut self, size: FrameSize) -> Result<Vec<u8>, PainterError> {
        let handle = &*self.gpu;
        if !self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.width == size.width && capture.height == size.height)
        {
            let texture = handle
                .device
                .create_texture(&vello::wgpu::TextureDescriptor {
                    label: Some("painter capture target"),
                    size: vello::wgpu::Extent3d {
                        width: size.width,
                        height: size.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: vello::wgpu::TextureDimension::D2,
                    format: vello::wgpu::TextureFormat::Rgba8Unorm,
                    usage: vello::wgpu::TextureUsages::RENDER_ATTACHMENT
                        | vello::wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
            let view = texture.create_view(&vello::wgpu::TextureViewDescriptor::default());
            let blitter = vello::wgpu::util::TextureBlitter::new(
                &handle.device,
                vello::wgpu::TextureFormat::Rgba8Unorm,
            );
            self.capture = Some(CaptureTarget {
                width: size.width,
                height: size.height,
                texture,
                view,
                blitter,
            });
        }
        let capture = self
            .capture
            .as_ref()
            .expect("the capture target was just ensured");
        let mut encoder =
            handle
                .device
                .create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
                    label: Some("painter capture blit"),
                });
        capture.blitter.copy(
            &handle.device,
            &mut encoder,
            &self.surface.target_view,
            &capture.view,
        );
        handle.queue.submit([encoder.finish()]);
        read_texture(
            &handle.device,
            &handle.queue,
            &capture.texture,
            size.width,
            size.height,
        )
        .map_err(|error| PainterError::Gpu(error.to_string()))
    }
}

impl Drop for WindowGraphics {
    fn drop(&mut self) {
        self.gpu.release(&mut self.filters, &mut self.vectors);
    }
}
