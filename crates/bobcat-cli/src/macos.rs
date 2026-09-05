//! The macOS windowed embedder.
//!
//! This file owns the embedder's share: the winit event loop, the window,
//! device metrics, input translation, the command prompt, PNG output — and,
//! because this is the thread that built the view, the frames themselves. An
//! OS fact goes in (`dispatch_input`, `resize`, `set_occluded`), and the turn
//! it opened ends in [`MacApplication::about_to_wait`], which runs the view's
//! own turn: `pump` draws the frame the view owes and hands back what the
//! realm had to say.
//!
//! The turn ends there and nowhere else. A frame asks `bobcat-main` for the
//! commit behind the next one, and `bobcat-main` answers through this very
//! [`ProxyWakeup`] — so drawing from inside an event relay would post into
//! the channel winit is still draining, and the run loop would never return
//! to `AppKit`. Winit's `RedrawRequested` is not relayed either.
//!
//! The loop always waits. What wakes it for a *frame* is this window's own
//! display: while [`bobcat_core::LynxView::owes_frame`] holds, a
//! [`crate::vsync::DisplayLink`] on the monitor the window is on posts one
//! wakeup per refresh, and stops the moment nothing is owed. The engine names
//! no interval and this file owns no timer — an animation runs at the rate
//! the display actually scans out, and a swap chain that had no image to give
//! is asked again one refresh later rather than on a guess.

use std::path::Path;
use std::sync::Arc;

use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
use bobcat_core::{DrawTarget, EngineEvent, EventRequester, LynxGroup, LynxView, StyleThreads};
use bobcat_resources::ViewResources;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::macos::MonitorHandleExtMacOS;
use winit::window::{Window, WindowId};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
use crate::screenshot::save_screenshot;
use crate::vsync::DisplayLink;

#[derive(Debug)]
enum UserEvent {
    Command(Command),
    Pump,
}

const MOUSE_POINTER_ID: u32 = u32::MAX;

/// The native reference embedder's policy for winit's abstract line wheel
/// unit. Core accepts wheel deltas in CSS pixels only.
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

/// This window's wakeup: a user event on winit's loop proxy, which is the
/// only thing that reaches the run loop from another thread. The Lynx main
/// thread holds it, and posting one is all it ever does with it.
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
    /// Declared before the window it draws into: the view owns the surface
    /// built from that window, so dropping the view first releases the
    /// surface before the last handle to the window goes — and the window
    /// itself is destroyed on this thread, which is the only one allowed to
    /// destroy it.
    view: Option<LynxView<ViewResources>>,
    /// This window's display clock, running only while a frame is owed.
    /// Declared before the view so it is stopped — and its callback proven
    /// finished — before the view it wakes goes away.
    vsync: Option<DisplayLink>,
    event_requester: Arc<ProxyWakeup>,
    window: Option<Arc<Window>>,
    console: Console,
    pointer: Option<PhysicalPosition<f64>>,
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
            view: None,
            vsync: None,
            event_requester,
            window: None,
            console,
            pointer: None,
            pressed: false,
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
        program.warn_about_compatibility_limits();

        // One construction: the window this view draws into, the input's
        // author CSS, and its entry MTS module are all arguments, so what
        // comes back is a view whose first commit is already styled and whose
        // surface already exists. The surface is built on this thread because
        // `AppKit` allows it nowhere else — and this is also the thread that
        // will draw into it, for the same reason.
        // The resource system completes image loads on its own workers; each
        // completion wakes the event loop the same way a commit does.
        let resources = program.resources({
            let requester = Arc::clone(&self.event_requester);
            move || requester.request_event()
        });
        // One window, so one group: its Lynx main thread and its Stylo workers
        // exist for this view alone, and the view is what keeps them alive.
        let group = pollster::block_on(LynxGroup::new(
            Arc::clone(&self.event_requester),
            StyleThreads::Auto,
        ))
        .map_err(|source| CliError::StartView {
            input: program.input.clone(),
            source,
        })?;
        let view = pollster::block_on(group.create_lynx_view(
            css_width,
            css_height,
            scale_factor,
            DrawTarget::window(Arc::clone(window)),
            resources.builder(),
            program.sources(),
        ))
        .map_err(|source| CliError::StartView {
            input: program.input,
            source,
        })?;

        self.view = Some(view);
        // The display this window is on is the clock its frames run at. A
        // monitor winit cannot name, or a link CoreVideo will not open,
        // leaves the window on the engine's own wakeups: it still paints
        // every commit, it just cannot pace an animation itself.
        self.vsync = window.current_monitor().and_then(|monitor| {
            let requester = Arc::clone(&self.event_requester);
            DisplayLink::new(monitor.native_id(), move || requester.request_event())
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
        let scale = self.window().map_or(1.0, |window| window.scale_factor());
        self.dispatch(InputEvent::wheel(point, wheel_delta_css(delta, scale)));
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

    /// Runs the view's turn: it draws the frame the view owes — which is
    /// where this thread waits for the display — and hands back the
    /// lifecycle events that turn produced.
    fn serve(&mut self, event_loop: &ActiveEventLoop) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let mut fatal = None;
        for event in view.pump() {
            match event {
                EngineEvent::ScriptRunError(source) if fatal.is_none() => {
                    fatal = Some(CliError::Script {
                        input: self.input.clone(),
                        source,
                    });
                }
                // The view cannot reach the screen again, so the window has
                // nothing left to show.
                EngineEvent::RenderFailed(error) if fatal.is_none() => {
                    fatal = Some(CliError::Engine(error));
                }
                // Not fatal — the realm survives it and later events are
                // still delivered — so it is reported and the window stays up.
                EngineEvent::ListenerFailed(error) => {
                    eprintln!("event listener failed: {error}");
                }
                // The same standing: only that one timer's turn was lost.
                EngineEvent::TimerFailed(error) => {
                    eprintln!("timer callback failed: {error}");
                }
                _ => {}
            }
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
                if let Some(view) = self.view.as_mut() {
                    view.set_occluded(occluded);
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
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.fail(event_loop, error);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Command(command) => self.command(event_loop, command),
            // The wakeup itself. Answering it is `about_to_wait`'s job; this
            // arm exists only because a wakeup had to be shaped as an event.
            UserEvent::Pump => {}
        }
    }

    /// The turn ends here: run the view's turn, then decide how long to
    /// sleep.
    ///
    /// Drawing belongs here and not in the event relays, for two reasons. A
    /// turn's whole event burst has been handed over by now, so a wheel
    /// flurry draws one frame carrying its sum. And a frame that asks
    /// `bobcat-main` for the next commit is answered through this loop's own
    /// proxy, which winit drains by iterating until the channel is empty —
    /// from inside that drain the run loop would never return to `AppKit`,
    /// while a wakeup posted from here simply opens the next turn.
    ///
    /// The loop then always waits: the only thing that asks for a frame is
    /// the display itself. While the view owes one, the display link posts a
    /// wakeup per refresh; the turn that answers it draws, and the turn that
    /// finds nothing owed stops the link.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // A turn that quit opened draws nothing: this thread is the one
        // taking the window down, and a frame presented into a surface being
        // torn down buys nothing. An event the engine queued in that same
        // turn goes with it, exactly as it does when the window's close
        // button is what ends the run.
        if event_loop.exiting() {
            return;
        }
        self.serve(event_loop);
        let owed = self.view.as_ref().is_some_and(LynxView::owes_frame);
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
