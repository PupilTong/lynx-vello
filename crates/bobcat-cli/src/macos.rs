//! The macOS windowed embedder.
//!
//! This file owns the embedder's share and nothing more: the winit event
//! loop, the window, device metrics, input translation, the command prompt,
//! and PNG output. Every handler is a relay into
//! [`bobcat_core::engine::Engine`] — an OS fact goes in
//! (`dispatch_input`, `resize`, `notify_redraw`, `pump`), and the engine
//! decides what the pipeline does with it. The engine owns the Lynx main
//! thread (the script realm) and shares the element tree with it behind
//! one lock; presentation and vsync stay on this thread. The capabilities
//! the engine schedules through (`request_redraw`, `pre_present_notify`,
//! the event-loop wakeup) are handed over once at attach time.

use std::path::Path;
use std::sync::Arc;

use bobcat_core::engine::{Engine, EngineEvent, FrameSize, WindowHooks};
use bobcat_core::lynx_element::dom::Point2D;
use bobcat_core::lynx_element::dom::input::{DeltaMode, InputEvent, PointerKind, PointerPhase};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
use crate::screenshot::save_screenshot;

#[derive(Debug)]
enum UserEvent {
    /// A console command from the stdin thread.
    Command(Command),
    /// An engine-owned thread has messages waiting; the engine must be
    /// pumped on this thread.
    Pump,
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

    let mut application = MacApplication::new(program, options, console, event_loop.create_proxy());
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
    input: String,
    initial_width: f32,
    initial_height: f32,
    engine: Option<Engine>,
    window: Option<Arc<Window>>,
    /// The handle the engine's wakeup capability posts back through.
    proxy: EventLoopProxy<UserEvent>,
    console: Console,
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
            .field("input", &self.input)
            .field("initial_width", &self.initial_width)
            .field("initial_height", &self.initial_height)
            .field("occluded", &self.occluded)
            .field("has_error", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

impl MacApplication {
    fn new(
        program: Program,
        options: &Options,
        console: Console,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        Self {
            input: program.input.clone(),
            program: Some(program),
            initial_width: options.viewport_width,
            initial_height: options.viewport_height,
            engine: None,
            window: None,
            proxy,
            console,
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
        program.warn_about_dropped_author_rules();

        let mut engine = Engine::new(program.config, css_width, css_height, scale_factor)?;
        // The engine builds the GPU surface here and presents from this
        // thread — vsync interacts with the OS only inside its redraw
        // relay. The three capabilities are the OS mechanisms it schedules
        // through.
        let request_window = Arc::clone(&window);
        let present_window = Arc::clone(&window);
        let render_wakeup = self.proxy.clone();
        engine.attach_window(
            Arc::clone(&window),
            FrameSize {
                width: physical_size.width,
                height: physical_size.height,
            },
            WindowHooks {
                request_frame: Box::new(move || request_window.request_redraw()),
                pre_present: Box::new(move || present_window.pre_present_notify()),
                wakeup: Box::new(move || {
                    let _ = render_wakeup.send_event(UserEvent::Pump);
                }),
            },
        )?;

        // The script boots concurrently on the engine's thread: the window is
        // already live, and the first committed batch triggers the first real
        // frame. Until then the engine paints the bare page.
        let script_wakeup = self.proxy.clone();
        engine.spawn_script(program.source, move || {
            let _ = script_wakeup.send_event(UserEvent::Pump);
        })?;

        self.engine = Some(engine);
        self.window = Some(window);
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
        self.engine
            .as_mut()
            .expect("the engine is installed with the window")
            .resize(css_width, css_height, scale_factor)?;
        Ok(())
    }

    fn command(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::Continue => {
                println!("Continuing with display vsync.");
                if let Some(engine) = &self.engine {
                    engine.refresh();
                }
            }
            Command::Pause => {
                println!("The window repaints only on new frames; nothing to pause.");
            }
            Command::Frame => {
                if let Some(engine) = &self.engine {
                    engine.refresh();
                }
                println!("Rendering one frame.");
            }
            Command::Screenshot(path) => self.screenshot(&path),
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

    /// Asks the engine for the current frame's pixels; writing the PNG is
    /// this embedder's IO.
    fn screenshot(&mut self, path: &Path) {
        let Some(engine) = self.engine.as_mut() else {
            eprintln!("bobcat: no window yet to capture");
            return;
        };
        // A screenshot failure must not tear down the session: report it at
        // the prompt like any other bad command.
        let saved = engine
            .capture()
            .map_err(CliError::Engine)
            .and_then(|shot| save_screenshot(path, shot.size, &shot.pixels));
        if let Err(error) = saved {
            eprintln!("bobcat: {error}");
        }
    }

    /// Relays one already-translated input event to the engine.
    fn dispatch(&mut self, event: InputEvent) {
        if let Some(engine) = self.engine.as_mut() {
            engine.dispatch_input(event);
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
                if !occluded && let Some(engine) = &self.engine {
                    engine.refresh();
                }
                Ok(())
            }
            WindowEvent::RedrawRequested => match self.engine.as_mut() {
                Some(engine) => engine.notify_redraw().map_err(CliError::Engine),
                None => Ok(()),
            },
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
            UserEvent::Pump => {
                let Some(engine) = self.engine.as_mut() else {
                    return;
                };
                // A successfully finished script needs no reaction, and
                // future lifecycle events default to none.
                for engine_event in engine.pump() {
                    if let EngineEvent::ScriptFinished(Err(source)) = engine_event {
                        let input = self.input.clone();
                        self.fail(event_loop, CliError::Script { input, source });
                    }
                }
            }
        }
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
