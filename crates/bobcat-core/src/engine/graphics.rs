//! The engine-owned window presentation stack.
//!
//! Created on the embedder's thread (the one place macOS guarantees layer
//! setup works), then moved whole to the render thread it lives on. The
//! render thread drains a latest-wins mailbox of prepared scenes,
//! rasterizes, presents, and serves screenshot captures, so vsync waits and
//! GPU stalls never occupy the thread the engine runs on.

use std::mem;
use std::sync::mpsc;

use lynx_element::dom::render::gpu::{read_texture, render_params, renderer_options};
use lynx_element::dom::vello;
use lynx_element::dom::vello::peniko::Color;
use lynx_element::dom::vello::util::{RenderContext, RenderSurface};

use super::{EngineError, FrameSize, Screenshot, ScreenshotSink};

/// The draw target an embedder hands over with its window: anything wgpu
/// can build a surface on (a window handle behind `Arc`, a raw handle
/// pair, …).
pub type WindowTarget = vello::wgpu::SurfaceTarget<'static>;

/// One prepared frame crossing to the render thread: the cloned scene, its
/// physical target size, and any screenshot requests to serve from it.
pub(super) struct FrameJob {
    pub(super) scene: vello::Scene,
    pub(super) size: FrameSize,
    pub(super) screenshots: Vec<ScreenshotSink>,
}

pub(super) struct WindowGraphics {
    context: RenderContext,
    surface: RenderSurface<'static>,
    renderer: vello::Renderer,
    capture: Option<CaptureTarget>,
}

/// A `COPY_SRC` twin of the surface's render target, on the same device, so
/// screenshots read back exactly what the window pipeline rendered instead
/// of re-rendering on a second GPU stack.
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
    pub(super) fn new(target: WindowTarget, size: FrameSize) -> Result<Self, EngineError> {
        let mut context = RenderContext::new();
        let surface = pollster::block_on(context.create_surface(
            target,
            size.width.max(1),
            size.height.max(1),
            vello::wgpu::PresentMode::AutoVsync,
        ))
        .map_err(|error| EngineError::Render(error.to_string()))?;
        let handle = &context.devices[surface.dev_id];
        let renderer = vello::Renderer::new(&handle.device, renderer_options())
            .map_err(|error| EngineError::Render(error.to_string()))?;
        Ok(Self {
            context,
            surface,
            renderer,
            capture: None,
        })
    }

    fn resize(&mut self, size: FrameSize) {
        if size.width != 0
            && size.height != 0
            && (self.surface.config.width != size.width
                || self.surface.config.height != size.height)
        {
            self.context
                .resize_surface(&mut self.surface, size.width, size.height);
        }
    }

    fn render(
        &mut self,
        scene: &vello::Scene,
        size: FrameSize,
        pre_present: &(dyn Fn() + Send),
    ) -> Result<(), EngineError> {
        if self.surface.config.width != size.width || self.surface.config.height != size.height {
            self.resize(size);
        }
        let Self {
            context,
            surface,
            renderer,
            ..
        } = self;
        // Keep the retained target current even when the compositor cannot
        // provide a surface texture. Screenshots read this target directly.
        {
            let handle = &context.devices[surface.dev_id];
            renderer
                .render_to_texture(
                    &handle.device,
                    &handle.queue,
                    scene,
                    &surface.target_view,
                    &render_params(Color::WHITE, size.width, size.height),
                )
                .map_err(|error| EngineError::Render(error.to_string()))?;
        }
        let (surface_texture, reconfigure_after) = match surface.surface.get_current_texture() {
            vello::wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            vello::wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            vello::wgpu::CurrentSurfaceTexture::Timeout
            | vello::wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            vello::wgpu::CurrentSurfaceTexture::Outdated => {
                context.configure_surface(surface);
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
        pre_present();
        surface_texture.present();
        if reconfigure_after {
            context.configure_surface(surface);
        }
        Ok(())
    }

    /// Reads back the frame most recently rendered into the surface's target
    /// texture, on the window's own device, as tightly-packed RGBA8 pixels.
    fn capture_frame(&mut self, size: FrameSize) -> Result<Vec<u8>, EngineError> {
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

/// The render thread's whole life: block on the mailbox, drain it to the
/// newest frame (carrying screenshot requests forward — they want "the
/// current frame", and newer is more current), rasterize, present, capture.
///
/// Runs until the engine drops its sender (normal shutdown) or rendering
/// fails, which is reported through `fail`.
pub(super) fn render_thread(
    mut graphics: WindowGraphics,
    frames: &mpsc::Receiver<FrameJob>,
    pre_present: &(dyn Fn() + Send),
    fail: &dyn Fn(EngineError),
) {
    while let Ok(mut job) = frames.recv() {
        while let Ok(mut newer) = frames.try_recv() {
            let mut screenshots = mem::take(&mut job.screenshots);
            screenshots.append(&mut newer.screenshots);
            newer.screenshots = screenshots;
            job = newer;
        }
        if let Err(error) = graphics.render(&job.scene, job.size, pre_present) {
            fail(error);
            return;
        }
        if job.screenshots.is_empty() {
            continue;
        }
        // A screenshot failure must not tear down the session: the sinks
        // report it embedder-side, and one bad capture leaves later frames
        // unharmed.
        match graphics.capture_frame(job.size) {
            Ok(pixels) => {
                let screenshot = Screenshot {
                    size: job.size,
                    pixels,
                };
                for deliver in job.screenshots {
                    deliver(Ok(screenshot.clone()));
                }
            }
            Err(error) => {
                let message = error.to_string();
                for deliver in job.screenshots {
                    deliver(Err(EngineError::Render(message.clone())));
                }
            }
        }
    }
}
