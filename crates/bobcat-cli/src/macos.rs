use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use pulsar::gpu::Headless;
use pulsar::vello;
use pulsar::vello::peniko::Color;
use pulsar::vello::util::{RenderContext, RenderSurface};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::{FramePipeline, FrameSize, Program};
use crate::screenshot::write_png;

#[derive(Debug)]
enum UserEvent {
    Command(Command),
}

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
    capture_gpu: Option<Headless>,
    window: Option<Arc<Window>>,
    console: Console,
    pending_screenshots: Vec<PathBuf>,
    running: bool,
    occluded: bool,
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
            capture_gpu: None,
            window: None,
            console,
            pending_screenshots: Vec::new(),
            running: true,
            occluded: false,
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
        graphics.render(window, frame.scene, frame.size)?;

        if self.pending_screenshots.is_empty() {
            return Ok(());
        }
        let capture_gpu = match &mut self.capture_gpu {
            Some(gpu) => gpu,
            None => self
                .capture_gpu
                .insert(Headless::new().map_err(CliError::Gpu)?),
        };
        let pixels = capture_gpu
            .render(
                frame.scene,
                frame.size.width,
                frame.size.height,
                Color::WHITE,
            )
            .map_err(CliError::Gpu)?;
        for path in mem::take(&mut self.pending_screenshots) {
            write_png(&path, frame.size.width, frame.size.height, &pixels).map_err(|source| {
                CliError::Screenshot {
                    path: path.clone(),
                    source,
                }
            })?;
            println!("Saved screenshot to {}.", path.display());
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
        if self.running && !self.occluded {
            self.request_redraw();
        }
    }
}

struct WindowGraphics {
    context: RenderContext,
    surface: RenderSurface<'static>,
    renderer: vello::Renderer,
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
        let renderer = vello::Renderer::new(
            &handle.device,
            vello::RendererOptions {
                antialiasing_support: vello::AaSupport::area_only(),
                ..vello::RendererOptions::default()
            },
        )
        .map_err(|error| CliError::Render(error.to_string()))?;
        Ok(Self {
            context,
            surface,
            renderer,
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
                &vello::RenderParams {
                    base_color: Color::WHITE,
                    width: size.width,
                    height: size.height,
                    antialiasing_method: vello::AaConfig::Area,
                },
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
