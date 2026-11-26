//! The macOS windowed embedder.
//!
//! This file owns the embedder's share: the winit event loop, the window,
//! device metrics, input translation, the command prompt, PNG output — and,
//! because this is the thread that built both halves, the frames themselves.
//! An OS fact goes to the painter (`dispatch_input`, `resize`,
//! `set_occluded`), and the turn it opened ends in
//! [`MacApplication::about_to_wait`], which takes both turns in order: the
//! painter draws the frame it owes, then the view services its host's
//! resources and hands back what the realm had to say.
//!
//! The turn ends there and nowhere else. A frame asks `bobcat-main` for the
//! commit behind the next one, and `bobcat-main` answers through this very
//! [`ProxyWakeup`] — so drawing from inside an event relay would post into
//! the channel winit is still draining, and the run loop would never return
//! to `AppKit`. Winit's `RedrawRequested` is not relayed either.
//!
//! The loop always waits. What wakes it for a *frame* is this window's own
//! display: while [`bobcat_core::Painter::owes_frame`] holds, a
//! [`crate::cli::vsync::DisplayLink`] on the monitor the window is on posts one
//! wakeup per refresh, and stops the moment nothing is owed. A realm timer is
//! not this loop's to wait out — the engine waits its own out and wakes this
//! thread like any other publication. The engine names no interval for a
//! frame either: an animation runs at the rate the display actually scans
//! out, and a swap chain that had no image to give is asked again one refresh
//! later rather than on a guess.
//!
//! Text editing: the view's [`EngineEvent::TextInputStateChanged`] turns the
//! window's input method on while an editing host is focused and tells it
//! where the caret is; a key the input method did not consume carries the
//! text it typed (`KeyEvent.text`), and the input method's own events are
//! the view's editing commands ([`super::ime`], which records what winit
//! sends when).

use std::path::Path;
use std::sync::Arc;

use bobcat_core::editing::{Command as EditCommand, TextInputState};
use bobcat_core::input::{InputEvent, KeyPhase, Modifiers, Point2D, PointerKind, PointerPhase};
use bobcat_core::{DrawTarget, EngineEvent, EventRequester, LynxView, Painter, ScreenMetrics};
use bobcat_resources::ViewResources;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{
    ElementState, KeyEvent, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::monitor::MonitorHandle;
use winit::platform::macos::MonitorHandleExtMacOS;
use winit::window::{Window, WindowId};

use crate::cli::CliError;
use crate::cli::args::Options;
use crate::cli::command::{COMMAND_HELP, Command, Console, describe_text_input};
use crate::cli::ime::ImeBridge;
use crate::cli::page::Program;
use crate::cli::screenshot::save_screenshot;
use crate::cli::vsync::DisplayLink;

#[derive(Debug)]
enum UserEvent {
    Command(Command),
    Pump,
    Vsync,
}

const MOUSE_POINTER_ID: u32 = u32::MAX;

const TAP_POINTER_ID: u32 = 1;

const WHEEL_LINE_CSS_PX: f32 = 40.0;

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|error| CliError::Window(error.to_string()))?;
    let proxy = event_loop.create_proxy();
    let event_requester = Arc::new(ProxyWakeup(proxy.clone()));
    let console =
        Console::start(move |command| proxy.send_event(UserEvent::Command(command)).is_ok())
            .map_err(CliError::Console)?;
    println!("bobcat: macOS window starting; enter `help` for commands");
    console.prompt();

    let mut application = MacApplication::new(program, options, console, event_requester);
    event_loop
        .run_app(&mut application)
        .map_err(|error| CliError::Window(error.to_string()))?;
    match application.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Debug)]
struct ProxyWakeup(EventLoopProxy<UserEvent>);

impl EventRequester for ProxyWakeup {
    fn request_event(&self) {
        let _ = self.0.send_event(UserEvent::Pump);
    }
}

struct MacApplication {
    program: Option<Program>,
    input: String,
    initial_width: f32,
    initial_height: f32,
    locale: String,
    preload_scripts: Vec<String>,
    vsync: Option<DisplayLink>,
    painter: Option<Painter>,
    view: Option<LynxView<ViewResources>>,
    event_requester: Arc<ProxyWakeup>,
    window: Option<Arc<Window>>,
    console: Console,
    pointer: Option<PhysicalPosition<f64>>,
    pressed: bool,
    modifiers: ModifiersState,
    ime: ImeBridge,
    text_input: Option<TextInputState>,
    error: Option<CliError>,
}

impl std::fmt::Debug for MacApplication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MacApplication")
            .field("input", &self.input)
            .field("initial_width", &self.initial_width)
            .field("initial_height", &self.initial_height)
            .field("locale", &self.locale)
            .field("preload_scripts", &self.preload_scripts)
            .field("has_error", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

impl MacApplication {
    fn new(
        program: Program,
        options: &Options,
        console: Console,
        event_requester: Arc<ProxyWakeup>,
    ) -> Self {
        Self {
            input: program.input.clone(),
            program: Some(program),
            initial_width: options.viewport_width,
            initial_height: options.viewport_height,
            locale: options.locale.clone(),
            preload_scripts: options.preload_scripts.clone(),
            vsync: None,
            painter: None,
            view: None,
            event_requester,
            window: None,
            console,
            pointer: None,
            pressed: false,
            modifiers: ModifiersState::empty(),
            ime: ImeBridge::default(),
            text_input: None,
            error: None,
        }
    }

    fn window(&self) -> Option<&Arc<Window>> {
        self.window.as_ref()
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
        let window = self.window.insert(os);
        let program = self
            .program
            .take()
            .expect("the program is consumed by the first window only");

        let resources = program.resources({
            let requester = Arc::clone(&self.event_requester);
            move || requester.request_event()
        });
        let group = super::start_group(
            Arc::clone(&self.event_requester),
            &self.locale,
            &self.preload_scripts,
            &program.input,
        )?;
        let screen = window
            .current_monitor()
            .or_else(|| event_loop.primary_monitor())
            .as_ref()
            .map_or_else(
                || ScreenMetrics::for_viewport(css_width, css_height, scale_factor),
                screen_metrics,
            );
        let sources = program.sources(screen, &self.locale);
        let view = group
            .create_lynx_view(
                css_width,
                css_height,
                scale_factor,
                resources.builder(),
                Vec::new(),
                sources,
            )
            .map_err(|source| CliError::StartView {
                input: program.input.clone(),
                source,
            })?;
        let mut painter = pollster::block_on(Painter::new(
            DrawTarget::window(Arc::clone(window)),
            css_width,
            css_height,
            scale_factor,
        ))
        .map_err(CliError::Engine)?;
        painter.attach(&view).map_err(CliError::Engine)?;

        self.painter = Some(painter);
        self.view = Some(view);
        self.vsync = window.current_monitor().and_then(|monitor| {
            let requester = Arc::clone(&self.event_requester);
            DisplayLink::new(monitor.native_id(), move || {
                let _ = requester.0.send_event(UserEvent::Vsync);
            })
        });
        if self.vsync.is_none() {
            eprintln!("bobcat: no display link for this window; animations will not be paced");
        }
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
            viewport_metrics(physical_size, window.scale_factor());
        self.painter
            .as_mut()
            .expect("the painter is installed with the window")
            .resize(css_width, css_height, scale_factor)?;
        Ok(())
    }

    fn command(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::Continue => {
                println!("Continuing with display vsync.");
                if let Some(painter) = &self.painter {
                    painter.refresh();
                }
            }
            Command::Pause => {
                println!("The window repaints only on new frames; nothing to pause.");
            }
            Command::Frame => {
                if let Some(painter) = &self.painter {
                    painter.refresh();
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
            Command::Tap(x, y) => {
                for phase in [PointerPhase::Down, PointerPhase::Up] {
                    self.dispatch(InputEvent::pointer(
                        Point2D::new(x, y),
                        TAP_POINTER_ID,
                        PointerKind::Touch,
                        phase,
                    ));
                }
            }
            Command::Key { key, text } => {
                let key = Arc::<str>::from(key);
                for (phase, text) in [(KeyPhase::Down, text), (KeyPhase::Up, None)] {
                    self.dispatch(
                        InputEvent::key(phase, key.clone(), "", false, Modifiers::default())
                            .with_text(text.map(Into::into)),
                    );
                }
            }
            Command::Compose { text, selection } => {
                self.edit(EditCommand::SetComposition { text, selection });
            }
            Command::Commit(text) => self.edit(EditCommand::CommitText {
                text,
                relative_cursor: 0,
            }),
            Command::ShowTextInput => {
                println!("{}", describe_text_input(self.text_input.as_ref()));
            }
            Command::Help => println!("{COMMAND_HELP}"),
            Command::Quit => event_loop.exit(),
            Command::Invalid(message) => eprintln!("bobcat: {message}"),
        }
        self.console.prompt();
    }

    fn screenshot(&mut self, path: &Path) {
        let Some(painter) = self.painter.as_mut() else {
            eprintln!("bobcat: no window yet to capture");
            return;
        };
        let saved = painter
            .capture()
            .map_err(CliError::Engine)
            .and_then(|shot| save_screenshot(path, shot.size, &shot.pixels));
        if let Err(error) = saved {
            eprintln!("bobcat: {error}");
        }
    }

    fn dispatch(&mut self, event: InputEvent) {
        if let Some(painter) = self.painter.as_mut() {
            painter.dispatch_input(event);
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
            PointerKind::Mouse,
            phase,
        ));
    }

    fn wheel_event(&mut self, delta: MouseScrollDelta) {
        let Some(position) = self.pointer else { return };
        let Some(point) = self.css_point(position) else {
            return;
        };
        let scale = self.window().map_or(1.0, |window| window.scale_factor());
        self.dispatch(InputEvent::wheel(point, wheel_delta_css(delta, scale)));
    }

    fn key_event(&mut self, event: &KeyEvent) {
        let phase = match event.state {
            ElementState::Pressed => KeyPhase::Down,
            ElementState::Released => KeyPhase::Up,
        };
        let modifiers = Modifiers {
            alt: self.modifiers.alt_key(),
            ctrl: self.modifiers.control_key(),
            meta: self.modifiers.super_key(),
            shift: self.modifiers.shift_key(),
        };
        self.dispatch(
            InputEvent::key(
                phase,
                super::keys::key_value(&event.logical_key),
                super::keys::code_value(event.physical_key),
                event.repeat,
                modifiers,
            )
            .with_text(event.text.as_deref().map(Arc::from)),
        );
    }

    fn edit(&self, command: EditCommand) {
        if let Some(view) = &self.view
            && let Err(error) = view.edit(command)
        {
            eprintln!("bobcat: the view refused the edit: {error}");
        }
    }

    fn flush_ime(&mut self) {
        if let Some(command) = self.ime.flush() {
            self.edit(command);
        }
    }

    fn text_input_changed(&mut self, state: TextInputState) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if self.text_input.as_ref().map(|last| last.host) != Some(state.host) {
            self.ime.reset();
        }
        window.set_ime_allowed(state.host.is_some());
        if state.host.is_some() {
            let area = state.composition_bounds.first().unwrap_or(&state.caret);
            window.set_ime_cursor_area(
                LogicalPosition::new(f64::from(area.origin.x), f64::from(area.origin.y)),
                LogicalSize::new(
                    f64::from(area.size.width.max(1.0)),
                    f64::from(area.size.height),
                ),
            );
        }
        self.text_input = Some(state);
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
        let scale = self.window()?.scale_factor();
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

    fn serve(&mut self, event_loop: &ActiveEventLoop) {
        let mut fatal = None;
        if let Some(painter) = self.painter.as_mut()
            && let Err(error) = painter.pump()
        {
            fatal = Some(CliError::Engine(error));
        }
        let Some(view) = self.view.as_mut() else {
            if let Some(error) = fatal {
                self.fail(event_loop, error);
            }
            return;
        };
        let mut text_input = None;
        for event in view.pump() {
            match event {
                event if event.is_fatal() && fatal.is_none() => {
                    fatal = Some(CliError::from_fatal_event(&self.input, event));
                }
                EngineEvent::ScriptRunError(error) => {
                    eprintln!("script failed: {error}");
                }
                EngineEvent::ListenerFailed(error) => {
                    eprintln!("event listener failed: {error}");
                }
                EngineEvent::TimerFailed(error) => {
                    eprintln!("timer callback failed: {error}");
                }
                EngineEvent::WorkerThrew { source, error } => {
                    eprintln!("[{source}] worker threw: {error}");
                }
                EngineEvent::WorkerEnded { source, error } => {
                    eprintln!("[{source}] worker ended: {error}");
                }
                EngineEvent::ScriptReported {
                    source,
                    level,
                    message,
                }
                | EngineEvent::ConsoleMessage {
                    source,
                    level,
                    message,
                } => {
                    eprintln!("[{source}] [{level}] {message}");
                }
                EngineEvent::TextInputStateChanged(state) => text_input = Some(*state),
                _ => {}
            }
        }
        if let Some(state) = text_input {
            self.text_input_changed(state);
        }
        if let Some(error) = fatal {
            self.fail(event_loop, error);
        }
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
        if self.window().map(|window| window.id()) != Some(window_id) {
            return;
        }
        if !matches!(event, WindowEvent::Ime(_)) {
            self.flush_ime();
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
                    .inner_size();
                self.resize(size)
            }
            WindowEvent::Occluded(occluded) => {
                if let Some(painter) = self.painter.as_mut() {
                    painter.set_occluded(occluded);
                }
                Ok(())
            }
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
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                Ok(())
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.key_event(&event);
                Ok(())
            }
            WindowEvent::Ime(event) => {
                for command in self.ime.ime(event) {
                    self.edit(command);
                }
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
            UserEvent::Pump => {}
            UserEvent::Vsync => {
                if let Some(painter) = self.painter.as_mut() {
                    painter.vsync();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            return;
        }
        self.flush_ime();
        self.serve(event_loop);
        let owed = self.painter.as_ref().is_some_and(Painter::owes_frame);
        if let Some(vsync) = self.vsync.as_mut() {
            vsync.set_running(owed);
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

fn non_empty_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "wheel deltas are far inside f32 range"
)]
fn wheel_delta_css(delta: MouseScrollDelta, scale_factor: f64) -> (f32, f32) {
    match delta {
        MouseScrollDelta::PixelDelta(pixels) => (
            -(pixels.x / scale_factor) as f32,
            -(pixels.y / scale_factor) as f32,
        ),
        MouseScrollDelta::LineDelta(x, y) => (-x * WHEEL_LINE_CSS_PX, -y * WHEEL_LINE_CSS_PX),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "stylo and layout use f32 CSS coordinates; winit's finite display scale and the \
              16384px target cap are far inside f32's useful range"
)]
fn screen_metrics(monitor: &MonitorHandle) -> ScreenMetrics {
    let size = monitor.size();
    ScreenMetrics {
        pixel_ratio: monitor.scale_factor() as f32,
        pixel_width: f64::from(size.width) as f32,
        pixel_height: f64::from(size.height) as f32,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_deltas_are_normalized_to_css_pixels() {
        assert_eq!(
            wheel_delta_css(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(20.0, -10.0)),
                2.0,
            ),
            (-10.0, 5.0)
        );
        assert_eq!(
            wheel_delta_css(MouseScrollDelta::LineDelta(2.0, -3.0), 2.0),
            (-80.0, 120.0)
        );
    }
}
