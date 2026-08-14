//! The engine-owned window presentation stack.
//!
//! Lives on the embedder's main thread beside its event loop — presentation
//! and vsync interact with the OS only here. Rendering into the retained
//! target texture and presenting it are separate steps: the target is
//! re-rendered only when a new scene was produced, while surface
//! invalidation and re-exposure re-present the retained target with a blit
//! alone.

#[cfg(not(target_arch = "wasm32"))]
use dom::render::gpu::read_texture;
use dom::render::gpu::{render_params, renderer_options};
use dom::vello;
use dom::vello::peniko::Color;
use dom::vello::util::{RenderContext, RenderSurface};

use super::{EngineError, FrameRequester, FrameSize};

pub type WindowTarget<'window> = vello::wgpu::SurfaceTarget<'window>;

pub(super) struct WindowGraphics<'window> {
    context: RenderContext,
    surface: RenderSurface<'window>,
    renderer: vello::Renderer,
    #[cfg(not(target_arch = "wasm32"))]
    capture: Option<CaptureTarget>,
    rendered: Option<FrameSize>,
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

impl std::fmt::Debug for WindowGraphics<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowGraphics")
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl<'window> WindowGraphics<'window> {
    pub(super) async fn new(
        target: impl Into<WindowTarget<'window>>,
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
            #[cfg(not(target_arch = "wasm32"))]
            capture: None,
            rendered: None,
        })
    }

    pub(super) fn rendered_at(&self, size: FrameSize) -> bool {
        self.rendered == Some(size)
    }

    pub(super) fn render_to_target(
        &mut self,
        scene: &vello::Scene,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        if size.width != 0
            && size.height != 0
            && (self.surface.config.width != size.width
                || self.surface.config.height != size.height)
        {
            self.context
                .resize_surface(&mut self.surface, size.width, size.height);
            self.rendered = None;
        }
        let handle = &self.context.devices[self.surface.dev_id];
        self.renderer
            .render_to_texture(
                &handle.device,
                &handle.queue,
                scene,
                &self.surface.target_view,
                &render_params(Color::WHITE, size.width, size.height),
            )
            .map_err(|error| EngineError::Render(error.to_string()))?;
        self.rendered = Some(size);
        Ok(())
    }

    /// Presents the retained target: acquires the surface texture, blits,
    /// notifies the window just before presenting, and presents. Called
    /// outside the tree lock — the vsync wait must not block anyone.
    pub(super) fn present(&mut self, frames: &impl FrameRequester) -> Result<(), EngineError> {
        let Self {
            context, surface, ..
        } = self;
        let (surface_texture, reconfigure_after) = match surface.surface.get_current_texture() {
            vello::wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            vello::wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            vello::wgpu::CurrentSurfaceTexture::Timeout
            | vello::wgpu::CurrentSurfaceTexture::Occluded => {
                frames.request_frame();
                return Ok(());
            }
            vello::wgpu::CurrentSurfaceTexture::Outdated => {
                context.configure_surface(surface);
                frames.request_frame();
                return Ok(());
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
        frames.pre_present();
        handle.queue.present(surface_texture);
        if reconfigure_after {
            context.configure_surface(surface);
        }
        Ok(())
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
        .map_err(EngineError::Gpu)
    }
}
