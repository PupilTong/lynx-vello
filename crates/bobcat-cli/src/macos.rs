use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use bobcat_core::dom::input::{DeltaMode, InputEvent, PointerKind, PointerPhase};
use bobcat_core::dom::visual::Point2D;
use bobcat_core::pulsar::gpu::{read_texture, render_params, renderer_options};
use bobcat_core::pulsar::vello;
use bobcat_core::pulsar::vello::peniko::Color;
use bobcat_core::pulsar::vello::util::{RenderContext, RenderSurface};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::{FramePipeline, FrameSize, Program};
use crate::screenshot::save_screenshot;

#[derive(Debug)]
enum UserEvent {
    Command(Command),
}

/// The one pointer id every mouse gesture uses. Real touches carry winit's own
/// per-contact ids, which cannot collide with this because they start at 0 and
/// this is deliberately out of that range.
const MOUSE_POINTER_ID: u32 = u32::MAX;

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|error| CliError::Window(error.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let console =
        Console::start(move |command| proxy.send_event(UserEvent::Command(command)).is_ok())
            .map_err(CliError::Console)?;
    println!("bobcat: macOS window starting; enter `help` for commands");
    console.prompt();

    let mut application = MacApplication::new(program, options, console);
    event_loop
        .run_app(&mut application)
        .map_err(|error| CliError::Window(error.to_string()))?;
    match application.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct MacApplication {
    program: Option<Program>,
    initial_width: f32,
    initial_height: f32,
    pipeline: Option<FramePipeline>,
    graphics: Option<WindowGraphics>,
    window: Option<Arc<Window>>,
    console: Console,
    pending_screenshots: Vec<PathBuf>,
    running: bool,
    occluded: bool,
    /// Last known cursor position, in physical window pixels. Mouse events
    /// other than `CursorMoved` do not carry one.
    pointer: Option<PhysicalPosition<f64>>,
    /// Whether the left button is currently down.
    pressed: bool,
    error: Option<CliError>,
}

impl std::fmt::Debug for MacApplication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MacApplication")
            .field("initial_width", &self.initial_width)
            .field("initial_height", &self.initial_height)
            .field("running", &self.running)
            .field("occluded", &self.occluded)
            .field("pending_screenshots", &self.pending_screenshots.len())
            .field("has_error", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

impl MacApplication {
    fn new(program: Program, options: &Options, console: Console) -> Self {
        Self {
            program: Some(program),
            initial_width: options.viewport_width,
            initial_height: options.viewport_height,
            pipeline: None,
            graphics: None,
            window: None,
            console,
            pending_screenshots: Vec::new(),
            running: true,
            occluded: false,
            pointer: None,
            pressed: false,
            error: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), CliError> {
        if self.window.is_some() {
            return Ok(());
        }
        let attributes = Window::default_attributes()
            .with_title("Bobcat")
            .with_inner_size(LogicalSize::new(
                f64::from(self.initial_width),
                f64::from(self.initial_height),
            ));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|error| CliError::Window(error.to_string()))?,
        );
        let physical_size = non_empty_size(window.inner_size());
        let (css_width, css_height, scale_factor) =
            viewport_metrics(physical_size, window.scale_factor());
        let program = self
            .program
            .take()
            .expect("the program is consumed by the first window only");
        let pipeline = program.boot(css_width, css_height, scale_factor)?;
        let graphics = WindowGraphics::new(Arc::clone(&window), physical_size)?;

        self.pipeline = Some(pipeline);
        self.graphics = Some(graphics);
        self.window = Some(window);
        self.request_redraw();
        Ok(())
    }

    fn resize(&mut self, physical_size: PhysicalSize<u32>) -> Result<(), CliError> {
        if physical_size.width == 0 || physical_size.height == 0 {
            return Ok(());
        }
        let window = self
            .window
            .as_ref()
            .expect("resize events arrive only after window creation");
        let (css_width, css_height, scale_factor) =
            viewport_metrics(physical_size, window.scale_factor());
        self.pipeline
            .as_mut()
            .expect("the pipeline is installed with the window")
            .resize(css_width, css_height, scale_factor)?;
        self.graphics
            .as_mut()
            .expect("graphics are installed with the window")
            .resize(physical_size);
        self.request_redraw();
        Ok(())
    }

    fn redraw(&mut self) -> Result<(), CliError> {
        let pipeline = self
            .pipeline
            .as_mut()
            .expect("redraw events arrive only after initialization");
        let graphics = self
            .graphics
            .as_mut()
            .expect("redraw events arrive only after initialization");
        let window = self
            .window
            .as_ref()
            .expect("redraw events arrive only after initialization");
        let frame = pipeline.prepare_frame();
        let scene = frame.scene();
        graphics.render(window, &scene, frame.size)?;
        drop(scene);

        if self.pending_screenshots.is_empty() {
            return Ok(());
        }
        // A screenshot failure must not tear down the session: report it at
        // the prompt like any other bad command, and let one bad path leave
        // the other queued captures unharmed.
        let paths = mem::take(&mut self.pending_screenshots);
        match graphics.capture_frame(frame.size) {
            Ok(pixels) => {
                for path in paths {
                    if let Err(error) = save_screenshot(&path, frame.size, &pixels) {
                        eprintln!("bobcat: {error}");
                    }
                }
            }
            Err(error) => eprintln!("bobcat: screenshot capture failed: {error}"),
        }
        Ok(())
    }

    fn command(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::Continue => {
                self.running = true;
                println!("Continuing with display vsync.");
                self.request_redraw();
            }
            Command::Pause => {
                self.running = false;
                println!("Frame clock paused.");
            }
            Command::Frame => {
                self.request_redraw();
                println!("Rendering one frame.");
            }
            Command::Screenshot(path) => {
                self.pending_screenshots.push(path);
                self.request_redraw();
            }
            Command::SetVsync(_) => {
                eprintln!(
                    "bobcat: `set vsync` controls the synthetic headless clock; headed mode uses \
                     the display's vsync"
                );
            }
            Command::ShowVsync => println!("Headed mode uses the display's vsync."),
            Command::Help => println!("{COMMAND_HELP}"),
            Command::Quit => event_loop.exit(),
            Command::Invalid(message) => eprintln!("bobcat: {message}"),
        }
        self.console.prompt();
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Feeds one already-built input event in and repaints if it moved
    /// anything. `ControlFlow::Wait` means nothing else would ask.
    fn dispatch(&mut self, event: InputEvent) {
        let Some(pipeline) = self.pipeline.as_mut() else {
            return;
        };
        pipeline.handle_input(event);
        if pipeline.needs_frame() {
            self.request_redraw();
        }
    }

    /// A mouse pointer event at the last known cursor position.
    ///
    /// A left-button drag is reported as a `Pen` rather than a `Mouse`: the DOM
    /// deliberately does not drag-scroll for a mouse (browsers do not either —
    /// a mouse scrolls with its wheel), and on a laptop without a touchscreen a
    /// click-drag is the only way to try the touch path at all. Wheel and
    /// trackpad scrolling still arrive as real wheel events below.
    fn pointer_event(&mut self, phase: PointerPhase) {
        let Some(position) = self.pointer else { return };
        // Moves only matter while a gesture is in flight; hover has no consumer.
        if phase == PointerPhase::Move && !self.pressed {
            return;
        }
        let Some(point) = self.css_point(position) else {
            return;
        };
        self.dispatch(InputEvent::pointer(
            point,
            MOUSE_POINTER_ID,
            PointerKind::Pen,
            phase,
        ));
    }

    fn wheel_event(&mut self, delta: MouseScrollDelta) {
        let Some(position) = self.pointer else { return };
        let Some(point) = self.css_point(position) else {
            return;
        };
        // Trackpads and high-resolution wheels report pixels; a notched wheel
        // reports lines, which is exactly what `DeltaMode::Line` is for.
        // Both invert: a wheel scrolled away from the user moves the reading
        // position forward, which is `+deltaY`.
        let event = match delta {
            MouseScrollDelta::PixelDelta(pixels) => {
                let scale = self
                    .window
                    .as_ref()
                    .map_or(1.0, |window| window.scale_factor());
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "wheel deltas are far inside f32 range"
                )]
                InputEvent::wheel(
                    point,
                    (-(pixels.x / scale) as f32, -(pixels.y / scale) as f32),
                )
            }
            MouseScrollDelta::LineDelta(x, y) => {
                InputEvent::wheel_with_mode(point, (-x, -y), DeltaMode::Line)
            }
        };
        self.dispatch(event);
    }

    fn touch_event(&mut self, touch: Touch) {
        let phase = match touch.phase {
            TouchPhase::Started => PointerPhase::Down,
            TouchPhase::Moved => PointerPhase::Move,
            TouchPhase::Ended => PointerPhase::Up,
            TouchPhase::Cancelled => PointerPhase::Cancel,
        };
        let Some(point) = self.css_point(touch.location) else {
            return;
        };
        #[allow(
            clippy::cast_possible_truncation,
            reason = "winit touch ids are per-contact counters"
        )]
        let id = touch.id as u32;
        self.dispatch(InputEvent::pointer(point, id, PointerKind::Touch, phase));
    }

    /// A physical window position as the viewport CSS-px point the engine
    /// hit-tests with. The window's scale factor is the only conversion: both
    /// spaces share an origin at the window's top-left content corner.
    fn css_point(&self, physical: PhysicalPosition<f64>) -> Option<Point2D<f32>> {
        let scale = self.window.as_ref()?.scale_factor();
        #[allow(
            clippy::cast_possible_truncation,
            reason = "window coordinates are far inside f32 range"
        )]
        Some(Point2D::new(
            (physical.x / scale) as f32,
            (physical.y / scale) as f32,
        ))
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: CliError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
        event_loop.exit();
    }
}

impl ApplicationHandler<UserEvent> for MacApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.initialize(event_loop) {
            self.fail(event_loop, error);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(|window| window.id()) != Some(window_id) {
            return;
        }
        let result = match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => self.resize(size),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = self
                    .window
                    .as_ref()
                    .expect("the event belongs to the current window")
                    .inner_size();
                self.resize(size)
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded {
                    self.request_redraw();
                }
                Ok(())
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = Some(position);
                self.pointer_event(PointerPhase::Move);
                Ok(())
            }
            WindowEvent::CursorLeft { .. } => {
                // The button may well come up outside the window; end the
                // gesture rather than leave it latched forever.
                self.pointer_event(PointerPhase::Cancel);
                self.pointer = None;
                self.pressed = false;
                Ok(())
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    self.pressed = state == ElementState::Pressed;
                    self.pointer_event(if self.pressed {
                        PointerPhase::Down
                    } else {
                        PointerPhase::Up
                    });
                }
                Ok(())
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.wheel_event(delta);
                Ok(())
            }
            WindowEvent::Touch(touch) => {
                self.touch_event(touch);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.fail(event_loop, error);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Command(command) => self.command(event_loop, command),
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Re-request only while the document has something new to paint: a
        // static scene must not turn `ControlFlow::Wait` into a permanent
        // full-refresh render loop.
        if self.running
            && !self.occluded
            && self
                .pipeline
                .as_ref()
                .is_some_and(FramePipeline::needs_frame)
        {
            self.request_redraw();
        }
    }
}

struct WindowGraphics {
    context: RenderContext,
    surface: RenderSurface<'static>,
    renderer: vello::Renderer,
    capture: Option<CaptureTarget>,
}

/// A `COPY_SRC` twin of the surface's render target, on the same device, so
/// screenshots read back exactly what the window pipeline rendered instead of
/// re-rendering on a second GPU stack.
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
    fn new(window: Arc<Window>, size: PhysicalSize<u32>) -> Result<Self, CliError> {
        let mut context = RenderContext::new();
        let surface = pollster::block_on(context.create_surface(
            window,
            size.width,
            size.height,
            vello::wgpu::PresentMode::AutoVsync,
        ))
        .map_err(|error| CliError::Render(error.to_string()))?;
        let handle = &context.devices[surface.dev_id];
        let renderer = vello::Renderer::new(&handle.device, renderer_options())
            .map_err(|error| CliError::Render(error.to_string()))?;
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
    ) -> Result<(), CliError> {
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
                return Err(CliError::Render(
                    "the macOS window surface was lost".to_owned(),
                ));
            }
            vello::wgpu::CurrentSurfaceTexture::Validation => {
                return Err(CliError::Render(
                    "surface acquisition raised a wgpu validation error".to_owned(),
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
            .map_err(|error| CliError::Render(error.to_string()))?;
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

    /// Reads back the frame most recently rendered into the surface's target
    /// texture, on the window's own device, as tightly-packed RGBA8 pixels.
    fn capture_frame(&mut self, size: FrameSize) -> Result<Vec<u8>, CliError> {
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
        .map_err(CliError::Gpu)
    }
}

fn non_empty_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "stylo and layout use f32 CSS coordinates; winit's finite display scale and the \
              16384px target cap are far inside f32's useful range"
)]
fn viewport_metrics(size: PhysicalSize<u32>, scale_factor: f64) -> (f32, f32, f32) {
    (
        (f64::from(size.width) / scale_factor) as f32,
        (f64::from(size.height) / scale_factor) as f32,
        scale_factor as f32,
    )
}
