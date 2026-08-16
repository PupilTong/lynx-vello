//! The macOS windowed embedder.
//!
//! This file owns the embedder's share and nothing more: the winit event
//! loop, the window, device metrics, input translation, the command prompt,
//! and PNG output. Every handler is a relay into [`bobcat_core::LynxView`] —
//! an OS fact goes in
//! (`dispatch_input`, `resize`, `notify_redraw`, `pump`), and the engine
//! decides what the pipeline does with it. The engine owns the Lynx main
//! thread (the script realm) and shares the element tree with it behind
//! one lock; presentation and vsync stay on this thread. [`MacWindow`] is
//! the window it borrows at attach time: the draw target plus the two OS
//! mechanisms it schedules through (`request_redraw`, `pre_present_notify`).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use bobcat_core::input::{DeltaMode, InputEvent, Point2D, PointerKind, PointerPhase};
use bobcat_core::{
    EngineEvent, EventRequester, FrameRequester, FrameSize, LynxView, Window as EmbedderWindow,
    quickjs_engine_factory,
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
    Pump,
}

const MOUSE_POINTER_ID: u32 = u32::MAX;

struct MacWindow {
    os: Arc<Window>,
}

impl EmbedderWindow for MacWindow {
    type Target<'window> = &'window Window;
    type Frames = FrameRequests;

    fn target(&self) -> &Window {
        &self.os
    }

    fn frames(&self) -> FrameRequests {
        FrameRequests {
            os: Arc::clone(&self.os),
        }
    }
}

struct FrameRequests {
    os: Arc<Window>,
}

impl FrameRequester for FrameRequests {
    fn request_frame(&self) {
        self.os.request_redraw();
    }

    fn pre_present(&self) {
        self.os.pre_present_notify();
    }
}

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|error| CliError::Window(error.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let event_proxy = proxy.clone();
    let event_requester: Arc<dyn EventRequester> = Arc::new(move || {
        let _ = event_proxy.send_event(UserEvent::Pump);
    });
    let console =
        Console::start(move |command| proxy.send_event(UserEvent::Command(command)).is_ok())
            .map_err(CliError::Console)?;
    println!("bobcat: macOS window starting; enter `help` for commands");
    console.prompt();

    let window = OnceLock::new();
    let mut application = MacApplication::new(program, options, console, event_requester, &window);
    event_loop
        .run_app(&mut application)
        .map_err(|error| CliError::Window(error.to_string()))?;
    match application.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct MacApplication<'window> {
    program: Option<Program>,
    input: String,
    initial_width: f32,
    initial_height: f32,
    view: Option<LynxView<'window, MacWindow>>,
    event_requester: Arc<dyn EventRequester>,
    window: &'window OnceLock<MacWindow>,
    console: Console,
    occluded: bool,
    pointer: Option<PhysicalPosition<f64>>,
    pressed: bool,
    error: Option<CliError>,
}

impl std::fmt::Debug for MacApplication<'_> {
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

impl<'window> MacApplication<'window> {
    fn new(
        program: Program,
        options: &Options,
        console: Console,
        event_requester: Arc<dyn EventRequester>,
        window: &'window OnceLock<MacWindow>,
    ) -> Self {
        Self {
            input: program.input.clone(),
            program: Some(program),
            initial_width: options.viewport_width,
            initial_height: options.viewport_height,
            view: None,
            event_requester,
            window,
            console,
            occluded: false,
            pointer: None,
            pressed: false,
            error: None,
        }
    }

    fn window(&self) -> Option<&'window MacWindow> {
        self.window.get()
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), CliError> {
        if self.window().is_some() {
            return Ok(());
        }
        let attributes = Window::default_attributes()
            .with_title("Bobcat")
            .with_inner_size(LogicalSize::new(
                f64::from(self.initial_width),
                f64::from(self.initial_height),
            ));
        let os = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|error| CliError::Window(error.to_string()))?,
        );
        let physical_size = non_empty_size(os.inner_size());
        let (css_width, css_height, scale_factor) =
            viewport_metrics(physical_size, os.scale_factor());
        let window = self.window.get_or_init(|| MacWindow { os });
        let program = self
            .program
            .take()
            .expect("the program is consumed by the first window only");
        program.warn_about_unscoped_author_styles();

        let mut view = LynxView::new(
            program.config,
            program.resource_fetcher,
            quickjs_engine_factory(),
            Arc::clone(&self.event_requester),
            css_width,
            css_height,
            scale_factor,
        )?;
        view.attach_window(
            window,
            FrameSize {
                width: physical_size.width,
                height: physical_size.height,
            },
        )?;
        // The bundle's author CSS mounts before the script builds its tree, so
        // the first commit is already styled.
        if let Some(url) = &program.style_sheet_url {
            pollster::block_on(view.load_style_sheet(url.as_str())).map_err(|source| {
                CliError::LoadStyleSheet {
                    input: program.input.clone(),
                    source,
                }
            })?;
        }
        pollster::block_on(view.execute_script(program.script_url.as_str())).map_err(|source| {
            CliError::StartScript {
                input: program.input,
                source,
            }
        })?;

        self.view = Some(view);
        Ok(())
    }

    fn resize(&mut self, physical_size: PhysicalSize<u32>) -> Result<(), CliError> {
        if physical_size.width == 0 || physical_size.height == 0 {
            return Ok(());
        }
        let window = self
            .window()
            .expect("resize events arrive only after window creation");
        let (css_width, css_height, scale_factor) =
            viewport_metrics(physical_size, window.os.scale_factor());
        self.view
            .as_mut()
            .expect("the view is installed with the window")
            .resize(css_width, css_height, scale_factor)?;
        Ok(())
    }

    fn command(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::Continue => {
                println!("Continuing with display vsync.");
                if let Some(view) = &self.view {
                    view.refresh();
                }
            }
            Command::Pause => {
                println!("The window repaints only on new frames; nothing to pause.");
            }
            Command::Frame => {
                if let Some(view) = &self.view {
                    view.refresh();
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

    fn screenshot(&mut self, path: &Path) {
        let Some(view) = self.view.as_mut() else {
            eprintln!("bobcat: no window yet to capture");
            return;
        };
        let saved = view
            .capture()
            .map_err(CliError::Engine)
            .and_then(|shot| save_screenshot(path, shot.size, &shot.pixels));
        if let Err(error) = saved {
            eprintln!("bobcat: {error}");
        }
    }

    fn dispatch(&mut self, event: InputEvent) {
        if let Some(view) = self.view.as_mut() {
            view.dispatch_input(event);
        }
    }

    fn pointer_event(&mut self, phase: PointerPhase) {
        let Some(position) = self.pointer else { return };
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
        let event = match delta {
            MouseScrollDelta::PixelDelta(pixels) => {
                let scale = self.window().map_or(1.0, |window| window.os.scale_factor());
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

    fn css_point(&self, physical: PhysicalPosition<f64>) -> Option<Point2D<f32>> {
        let scale = self.window()?.os.scale_factor();
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

    fn pump(&mut self, event_loop: &ActiveEventLoop) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let error = view.pump().into_iter().find_map(|event| match event {
            EngineEvent::ScriptFinished(Err(source)) => Some(source),
            _ => None,
        });
        if let Some(source) = error {
            self.fail(
                event_loop,
                CliError::Script {
                    input: self.input.clone(),
                    source,
                },
            );
        }
    }
}

impl ApplicationHandler<UserEvent> for MacApplication<'_> {
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
        if self.window().map(|window| window.os.id()) != Some(window_id) {
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
                    .window()
                    .expect("the event belongs to the current window")
                    .os
                    .inner_size();
                self.resize(size)
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded && let Some(view) = &self.view {
                    view.refresh();
                }
                Ok(())
            }
            WindowEvent::RedrawRequested => match self.view.as_mut() {
                Some(view) => view.notify_redraw().map_err(CliError::Engine),
                None => Ok(()),
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = Some(position);
                self.pointer_event(PointerPhase::Move);
                Ok(())
            }
            WindowEvent::CursorLeft { .. } => {
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
        } else {
            self.pump(event_loop);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Command(command) => self.command(event_loop, command),
            UserEvent::Pump => {}
        }
        self.pump(event_loop);
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
