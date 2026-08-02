use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use bobcat_core::renderer::{
    DeltaMode, InputEvent, Point2D, PointerKind, PointerPhase, WindowRenderer,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
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
    println!(
        "bobcat: {} window starting; enter `help` for commands",
        std::env::consts::OS
    );
    console.prompt();

    let mut application = HeadedApplication::new(program, options, console);
    event_loop
        .run_app(&mut application)
        .map_err(|error| CliError::Window(error.to_string()))?;
    match application.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct HeadedApplication {
    program: Option<Program>,
    initial_width: f32,
    initial_height: f32,
    renderer: Option<WindowRenderer>,
    window: Option<Arc<Window>>,
    console: Console,
    pending_screenshots: Vec<PathBuf>,
    /// Last known cursor position, in physical window pixels. Mouse events
    /// other than `CursorMoved` do not carry one.
    pointer: Option<PhysicalPosition<f64>>,
    /// Whether the left button is currently down.
    pressed: bool,
    error: Option<CliError>,
}

impl std::fmt::Debug for HeadedApplication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeadedApplication")
            .field("initial_width", &self.initial_width)
            .field("initial_height", &self.initial_height)
            .field("pending_screenshots", &self.pending_screenshots.len())
            .field("has_error", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

impl HeadedApplication {
    fn new(program: Program, options: &Options, console: Console) -> Self {
        Self {
            program: Some(program),
            initial_width: options.viewport_width,
            initial_height: options.viewport_height,
            renderer: None,
            window: None,
            console,
            pending_screenshots: Vec::new(),
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
        let program = self
            .program
            .take()
            .expect("the program is consumed by the first window only");
        let renderer = WindowRenderer::new(program.into_render_program(), Arc::clone(&window))?;

        self.renderer = Some(renderer);
        self.window = Some(window);
        self.request_redraw();
        Ok(())
    }

    fn resize(&mut self, physical_size: PhysicalSize<u32>) -> Result<(), CliError> {
        if physical_size.width == 0 || physical_size.height == 0 {
            return Ok(());
        }
        self.renderer
            .as_mut()
            .expect("the renderer is installed with the window")
            .resize(physical_size)
            .map_err(CliError::from)
    }

    fn redraw(&mut self) -> Result<(), CliError> {
        if self.pending_screenshots.is_empty() {
            return self
                .renderer
                .as_mut()
                .expect("redraw events arrive only after initialization")
                .redraw()
                .map_err(CliError::from);
        }
        // A screenshot failure must not tear down the session: report it at
        // the prompt like any other bad command, and let one bad path leave
        // the other queued captures unharmed.
        let paths = mem::take(&mut self.pending_screenshots);
        let renderer = self
            .renderer
            .as_mut()
            .expect("redraw events arrive only after initialization");
        match renderer.capture() {
            Ok(frame) => {
                for path in paths {
                    if let Err(error) = save_screenshot(&path, frame.size(), frame.pixels()) {
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
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resume();
                }
                println!("Continuing with display vsync.");
            }
            Command::Pause => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.pause();
                }
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
        if let Some(renderer) = &self.renderer {
            renderer.request_frame();
        } else if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Feeds one already-built input event in and repaints if it moved
    /// anything. `ControlFlow::Wait` means nothing else would ask.
    fn dispatch(&mut self, event: InputEvent) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.handle_input(event);
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

impl ApplicationHandler<UserEvent> for HeadedApplication {
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
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_occluded(occluded);
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
        if let Some(renderer) = &self.renderer {
            renderer.about_to_wait();
        }
    }
}
