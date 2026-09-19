//! The view's window presentation stack.
//!
//! Built on the thread that owns the window, because creating a surface from
//! a window handle is a main-thread-only call on macOS — and that is the same
//! thread that draws, so it is never handed anywhere: every acquire, render
//! and present happens on the thread that built the view. Rendering into the
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

use dom::render::blur::FilterTextures;
#[cfg(not(target_arch = "wasm32"))]
use dom::render::gpu::read_texture;
use dom::render::gpu::{AtlasResidency, render_params, renderer_options};
use dom::vello::peniko::{Color, ImageData};
use dom::vello::util::{RenderContext, RenderSurface};
use dom::{CommittedFrame, ScrollSlot, Vector2D, vello};

use crate::view::{EngineError, FrameSize};

/// The draw target an embedder lends a view.
///
/// `'static` deliberately: the view owns the surface built from it for the
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

pub(crate) struct WindowGraphics {
    context: RenderContext,
    surface: RenderSurface<'static>,
    renderer: vello::Renderer,
    /// What that renderer's image atlas still holds; see
    /// [`dom::render::gpu::AtlasResidency`].
    atlas: AtlasResidency,
    /// The `filter: blur()` bakes of the frame this renderer last composed;
    /// see [`dom::render::blur::FilterTextures`].
    filters: FilterTextures,
    #[cfg(not(target_arch = "wasm32"))]
    capture: Option<CaptureTarget>,
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
    pub(crate) async fn new(
        target: impl Into<WindowTarget>,
        size: FrameSize,
    ) -> Result<Self, EngineError> {
        let mut context = RenderContext::new();
        let surface = context
            .create_surface(
                target,
                size.width.max(1),
                size.height.max(1),
                vello::wgpu::PresentMode::AutoVsync,
            )
            .await
            .map_err(|error| EngineError::Render(error.to_string()))?;
        let handle = &context.devices[surface.dev_id];
        let renderer = vello::Renderer::new(&handle.device, renderer_options())
            .map_err(|error| EngineError::Render(error.to_string()))?;
        Ok(Self {
            context,
            surface,
            renderer,
            atlas: AtlasResidency::default(),
            filters: FilterTextures::default(),
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
            self.context
                .resize_surface(&mut self.surface, size.width, size.height);
        }
    }

    /// Takes the swap-chain image this frame will be presented in, before any
    /// of the frame's work is done.
    ///
    /// The surface is brought to `size` first, so the image handed back is the
    /// one the coming render actually fits. This call is the frame's vsync
    /// wait; a caller that gets [`FrameAcquisition::Retry`] has no image this
    /// frame and must ask for another.
    pub(super) fn acquire(&mut self, size: FrameSize) -> Result<FrameAcquisition, EngineError> {
        self.configure_for(size);
        let Self {
            context, surface, ..
        } = self;
        let (texture, reconfigure_after) = match surface.surface.get_current_texture() {
            vello::wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            vello::wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            vello::wgpu::CurrentSurfaceTexture::Timeout
            | vello::wgpu::CurrentSurfaceTexture::Occluded => return Ok(FrameAcquisition::Retry),
            vello::wgpu::CurrentSurfaceTexture::Outdated => {
                context.configure_surface(surface);
                return Ok(FrameAcquisition::Retry);
            }
            vello::wgpu::CurrentSurfaceTexture::Lost => {
                return Err(EngineError::Render(
                    "the window surface was lost".to_owned(),
                ));
            }
            vello::wgpu::CurrentSurfaceTexture::Validation => {
                return Err(EngineError::Render(
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
    /// needs to keep this renderer's uploads truthful.
    pub(super) fn render_to_target(
        &mut self,
        scene: &vello::Scene,
        images: &[Option<ImageData>],
        size: FrameSize,
    ) -> Result<(), EngineError> {
        self.configure_for(size);
        // The filter bakes are override images this scene may draw, so the
        // residency has to see them too or a post-loss repair is missed.
        self.atlas
            .prepare_all(&mut self.renderer, scene, images, self.filters.images());
        let handle = &self.context.devices[self.surface.dev_id];
        self.renderer
            .render_to_texture(
                &handle.device,
                &handle.queue,
                scene,
                &self.surface.target_view,
                &render_params(Color::WHITE, size.width, size.height),
            )
            .map_err(|error| EngineError::Render(error.to_string()))
    }

    /// Bakes `frame`'s `filter: blur()` groups and `backdrop-filter`
    /// elements on the window's own device.
    ///
    /// Call before composing; a frame with no filter entry asks nothing of
    /// the device.
    pub(super) fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
    ) -> Result<&[Option<ImageData>], EngineError> {
        let Self {
            context,
            surface,
            renderer,
            atlas,
            filters,
            ..
        } = self;
        let handle = &context.devices[surface.dev_id];
        filters
            .prepare(
                renderer,
                &handle.device,
                &handle.queue,
                atlas,
                frame,
                images,
                offset_of,
                scroll_generation,
                animation_now,
            )
            .map_err(|error| EngineError::Gpu(error.to_string()))
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
        let Self {
            context, surface, ..
        } = self;
        let handle = &context.devices[surface.dev_id];
        let output_view = surface_texture
            .texture
            .create_view(&vello::wgpu::TextureViewDescriptor::default());
        let mut encoder =
            handle
                .device
                .create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
                    label: Some("bobcat window blit"),
                });
        surface.blitter.copy(
            &handle.device,
            &mut encoder,
            &surface.target_view,
            &output_view,
        );
        handle.queue.submit([encoder.finish()]);
        surface_texture.present();
        if reconfigure_after {
            context.configure_surface(surface);
        }
    }

    /// Reads back the frame most recently rendered into the surface's target
    /// texture, on the window's own device, as tightly-packed RGBA8 pixels.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn capture_frame(&mut self, size: FrameSize) -> Result<Vec<u8>, EngineError> {
        let handle = &self.context.devices[self.surface.dev_id];
        if !self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.width == size.width && capture.height == size.height)
        {
            let texture = handle
                .device
                .create_texture(&vello::wgpu::TextureDescriptor {
                    label: Some("bobcat capture target"),
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
                    label: Some("bobcat capture blit"),
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
        .map_err(|error| EngineError::Gpu(error.to_string()))
    }
}
