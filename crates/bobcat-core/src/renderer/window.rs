use std::sync::Arc;

use lynx_element::dom::pulsar::gpu::{read_texture, render_params, renderer_options};
use lynx_element::dom::pulsar::vello;
use lynx_element::dom::pulsar::vello::peniko::Color;
use lynx_element::dom::pulsar::vello::util::{RenderContext, RenderSurface};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use super::pipeline::{CapturedFrame, FrameSize, RenderProgram, RenderRuntime};
use super::{InputEvent, InputResponse, RenderError, Viewport};

/// Display-backed renderer for the supported desktop platforms (macOS and
/// Linux). It owns scene freshness checks, the Vello renderer, wgpu surface,
/// display-vsync present mode, and screenshot readback.
pub struct WindowRenderer {
    runtime: RenderRuntime,
    window: Arc<Window>,
    graphics: WindowGraphics,
    running: bool,
    occluded: bool,
}

impl std::fmt::Debug for WindowRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowRenderer")
            .field("runtime", &self.runtime)
            .field("window_id", &self.window.id())
            .field("running", &self.running)
            .field("occluded", &self.occluded)
            .finish_non_exhaustive()
    }
}

impl WindowRenderer {
    /// Boots a program from the native window's current CSS size and scale,
    /// then selects display-vsync presentation internally.
    pub fn new(program: RenderProgram, window: Arc<Window>) -> Result<Self, RenderError> {
        let size = non_empty_size(window.inner_size());
        let (width, height, scale) = viewport_metrics(size, window.scale_factor());
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(scale);
        let runtime = program.boot(viewport)?;
        let graphics = WindowGraphics::new(Arc::clone(&window), size)?;
        let renderer = Self {
            runtime,
            window,
            graphics,
            running: true,
            occluded: false,
        };
        renderer.request_frame();
        Ok(renderer)
    }

    #[must_use]
    pub fn window(&self) -> &Window {
        &self.window
    }

    /// Applies a native resize and schedules the resulting frame. Empty sizes
    /// are ignored while a window is minimized.
    pub fn resize(&mut self, size: PhysicalSize<u32>) -> Result<(), RenderError> {
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let (width, height, scale) = viewport_metrics(size, self.window.scale_factor());
        self.runtime.resize(width, height, scale)?;
        self.graphics.resize(size);
        self.request_frame();
        Ok(())
    }

    /// Routes input and requests a display frame only when the runtime changed
    /// visual state. The freshness predicate remains inside the engine.
    pub fn handle_input(&mut self, event: InputEvent) -> InputResponse {
        let response = self.runtime.handle_input(event);
        if self.runtime.needs_frame() {
            self.request_frame();
        }
        response
    }

    /// Handles one native redraw opportunity and presents through the private
    /// display-vsync surface.
    pub fn redraw(&mut self) -> Result<(), RenderError> {
        self.draw(false).map(|_| ())
    }

    /// Renders and reads back the current frame without exposing a texture,
    /// queue, or scene.
    pub fn capture(&mut self) -> Result<CapturedFrame, RenderError> {
        Ok(self.draw(true)?.expect("capture was requested"))
    }

    pub fn pause(&mut self) {
        self.running = false;
    }

    pub fn resume(&mut self) {
        self.running = true;
        self.request_frame();
    }

    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        if !occluded {
            self.request_frame();
        }
    }

    /// Schedules an explicit frame (for example, a debugger single-step).
    pub fn request_frame(&self) {
        self.window.request_redraw();
    }

    /// Called before the native event loop sleeps. Future animation and rAF
    /// mutations can make the retained frame stale without exposing that bit
    /// to the embedder; this method resolves it and requests display vsync.
    pub fn about_to_wait(&self) {
        if self.running && !self.occluded && self.runtime.needs_frame() {
            self.request_frame();
        }
    }

    fn draw(&mut self, capture: bool) -> Result<Option<CapturedFrame>, RenderError> {
        let frame = self.runtime.prepare_frame();
        let size = frame.size;
        let scene = frame.scene();
        self.graphics.render(&self.window, &scene, size)?;
        drop(scene);
        drop(frame);

        if !capture {
            return Ok(None);
        }
        let pixels = self.graphics.capture_frame(size)?;
        Ok(Some(CapturedFrame::new(size, pixels)))
    }
}

struct WindowGraphics {
    context: RenderContext,
    surface: RenderSurface<'static>,
    renderer: vello::Renderer,
    capture: Option<CaptureTarget>,
}

/// A `COPY_SRC` twin of the surface target on the same device. Explicit
/// captures read the exact displayed pipeline rather than rendering through a
/// second GPU stack.
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
    fn new(window: Arc<Window>, size: PhysicalSize<u32>) -> Result<Self, RenderError> {
        let mut context = RenderContext::new();
        let surface = pollster::block_on(context.create_surface(
            window,
            size.width,
            size.height,
            vello::wgpu::PresentMode::AutoVsync,
        ))
        .map_err(|error| RenderError::Backend(error.to_string()))?;
        let handle = &context.devices[surface.dev_id];
        let renderer = vello::Renderer::new(&handle.device, renderer_options())
            .map_err(|error| RenderError::Backend(error.to_string()))?;
        Ok(Self {
            context,
            surface,
            renderer,
            capture: None,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
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
        window: &Window,
        scene: &vello::Scene,
        size: FrameSize,
    ) -> Result<(), RenderError> {
        if self.surface.config.width != size.width || self.surface.config.height != size.height {
            self.resize(PhysicalSize::new(size.width, size.height));
        }
        let Self {
            context,
            surface,
            renderer,
            ..
        } = self;
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
                return Err(RenderError::Backend(
                    "the window surface was lost".to_owned(),
                ));
            }
            vello::wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::Backend(
                    "surface acquisition raised a validation error".to_owned(),
                ));
            }
        };
        let handle = &context.devices[surface.dev_id];
        renderer
            .render_to_texture(
                &handle.device,
                &handle.queue,
                scene,
                &surface.target_view,
                &render_params(Color::WHITE, size.width, size.height),
            )
            .map_err(|error| RenderError::Backend(error.to_string()))?;
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
        window.pre_present_notify();
        surface_texture.present();
        if reconfigure_after {
            context.configure_surface(surface);
        }
        Ok(())
    }

    fn capture_frame(&mut self, size: FrameSize) -> Result<Vec<u8>, RenderError> {
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
        .map_err(RenderError::from)
    }
}

fn non_empty_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "stylo/layout use f32 CSS coordinates and the 16384px target cap is far inside range"
)]
fn viewport_metrics(size: PhysicalSize<u32>, scale_factor: f64) -> (f32, f32, f32) {
    (
        (f64::from(size.width) / scale_factor) as f32,
        (f64::from(size.height) / scale_factor) as f32,
        scale_factor as f32,
    )
}
