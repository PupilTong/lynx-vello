//! The headless embedder: a synthetic vsync clock and a command prompt over
//! the view's offscreen output.
//!
//! The clock is this embedder's substitute for an OS display loop — it
//! relays ticks; whether a tick becomes GPU work is the view's decision
//! (`tick` renders only when the document changed). Screenshots come back as
//! pixels, and writing the PNG is this side's IO.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use bobcat_core::{EngineEvent, EventRequester, LynxView};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
use crate::screenshot::save_screenshot;

/// This embedder's wakeup: a `Pump` on the same channel the console and the
/// synthetic clock feed, so an engine wakeup is one more loop input.
#[derive(Debug)]
struct ChannelWakeup(mpsc::Sender<HostEvent>);

impl EventRequester for ChannelWakeup {
    fn request_event(&self) {
        let _ = self.0.send(HostEvent::Pump);
    }
}

/// The offscreen view this embedder drives, woken through its own channel.
type HeadlessView = LynxView<ChannelWakeup>;

pub(crate) fn run(program: &Program, options: &Options) -> Result<(), CliError> {
    program.warn_about_compatibility_limits();
    let (sender, receiver) = mpsc::channel();
    let event_requester = std::sync::Arc::new(ChannelWakeup(sender.clone()));
    // One construction: the input's author CSS and its entry MTS module are
    // this view's sources, so the first commit is already styled and the Lynx
    // main thread is running before anything else here happens.
    let mut view = pollster::block_on(HeadlessView::new(
        program.config,
        &program.resource_fetcher,
        event_requester,
        options.viewport_width,
        options.viewport_height,
        options.device_pixel_ratio,
        program.sources(),
    ))
    .map_err(|source| CliError::StartView {
        input: program.input.clone(),
        source,
    })?;
    view.attach_offscreen()?;

    view.tick(true)?;
    let console = Console::start(move |command| sender.send(HostEvent::Command(command)).is_ok())
        .map_err(CliError::Console)?;
    let mut clock = FrameClock::new(options.vsync_hz);
    let mut script = ScriptGate::default();
    let mut running = true;
    println!(
        "bobcat: headless renderer running at {} Hz; enter `help` for commands",
        clock.rate().get()
    );
    console.prompt();

    loop {
        let command = if let Some(command) = script.next_ready() {
            Some(command)
        } else if running {
            match receiver.recv_timeout(clock.time_until_tick()) {
                Ok(HostEvent::Command(command)) => Some(command),
                Ok(HostEvent::Pump) => {
                    if check_script(&mut view, &program.input)? {
                        script.finish();
                    }
                    None
                }
                Err(RecvTimeoutError::Timeout) => {
                    view.tick(false)?;
                    clock.advance();
                    None
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            match receiver.recv() {
                Ok(HostEvent::Command(command)) => Some(command),
                Ok(HostEvent::Pump) => {
                    if check_script(&mut view, &program.input)? {
                        script.finish();
                    }
                    None
                }
                Err(_) => return Ok(()),
            }
        };

        let Some(command) = command else {
            continue;
        };
        let Some(command) = script.accept(command) else {
            // Keep the console responsive while a render-dependent command
            // waits for the first script commit. Commands behind it remain
            // ordered, so `screenshot ...` followed by `quit` still writes
            // the requested frame. A leading `quit` remains immediate.
            console.prompt();
            continue;
        };
        if execute_command(command, &mut view, &mut clock, &mut running)? {
            return Ok(());
        }
        console.prompt();
    }
}

/// Executes one command and reports whether the render loop should exit.
fn execute_command(
    command: Command,
    view: &mut HeadlessView,
    clock: &mut FrameClock,
    running: &mut bool,
) -> Result<bool, CliError> {
    match command {
        Command::Continue => {
            *running = true;
            clock.restart();
            println!("Continuing at {} Hz.", clock.rate().get());
        }
        Command::Pause => {
            *running = false;
            println!("Frame clock paused.");
        }
        Command::Frame => {
            view.tick(true)?;
            clock.restart();
            println!("Rendered one frame.");
        }
        Command::Screenshot(path) => {
            let result = view
                .capture()
                .map_err(CliError::Engine)
                .and_then(|shot| save_screenshot(&path, shot.size, &shot.pixels));
            if let Err(error) = result {
                eprintln!("bobcat: {error}");
            }
            clock.restart();
        }
        Command::SetVsync(rate) => {
            clock.set_rate(rate);
            println!("Headless vsync is now {} Hz.", rate.get());
        }
        Command::ShowVsync => {
            println!("Headless vsync is {} Hz.", clock.rate().get());
        }
        Command::Help => println!("{COMMAND_HELP}"),
        Command::Quit => return Ok(true),
        Command::Invalid(message) => eprintln!("bobcat: {message}"),
    }
    Ok(false)
}

enum HostEvent {
    Command(Command),
    Pump,
}

fn check_script(view: &mut HeadlessView, input: &str) -> Result<bool, CliError> {
    let mut finished = false;
    for event in view.pump() {
        match event {
            EngineEvent::ScriptFinished => finished = true,
            EngineEvent::ScriptRunError(source) => {
                return Err(CliError::Script {
                    input: input.to_owned(),
                    source,
                });
            }
            // Not fatal: the walk went on and the realm is still usable, so
            // this is reported the way a browser console reports one rather
            // than by stopping the run.
            EngineEvent::ListenerFailed(error) => {
                eprintln!("event listener failed: {error}");
            }
            EngineEvent::RenderFailed(error) => return Err(CliError::Engine(error)),
            _ => {}
        }
    }
    Ok(finished)
}

/// Holds commands that would otherwise observe the initial blank target.
///
/// Only frame production and capture require the first script to finish. Once
/// one such command is waiting, later commands queue behind it to preserve the
/// console's input order. Before that point, including for a leading `quit`,
/// the console remains fully usable while the script thread is running.
#[derive(Debug, Default)]
struct ScriptGate {
    finished: bool,
    pending: VecDeque<Command>,
}

impl ScriptGate {
    fn finish(&mut self) {
        self.finished = true;
    }

    fn accept(&mut self, command: Command) -> Option<Command> {
        if self.finished || (self.pending.is_empty() && !requires_script(&command)) {
            Some(command)
        } else {
            self.pending.push_back(command);
            None
        }
    }

    fn next_ready(&mut self) -> Option<Command> {
        self.finished.then(|| self.pending.pop_front()).flatten()
    }
}

const fn requires_script(command: &Command) -> bool {
    matches!(command, Command::Frame | Command::Screenshot(_))
}

#[derive(Clone, Copy, Debug)]
struct FrameClock {
    rate: NonZeroU32,
    interval: Duration,
    next_tick: Instant,
}

impl FrameClock {
    fn new(rate: NonZeroU32) -> Self {
        let interval = interval(rate);
        Self {
            rate,
            interval,
            next_tick: Instant::now() + interval,
        }
    }

    const fn rate(self) -> NonZeroU32 {
        self.rate
    }

    fn set_rate(&mut self, rate: NonZeroU32) {
        self.rate = rate;
        self.interval = interval(rate);
        self.restart();
    }

    fn restart(&mut self) {
        self.next_tick = Instant::now() + self.interval;
    }

    fn time_until_tick(self) -> Duration {
        self.next_tick.saturating_duration_since(Instant::now())
    }

    fn advance(&mut self) {
        self.next_tick += self.interval;
        let now = Instant::now();
        if self.next_tick <= now {
            self.next_tick = now + self.interval;
        }
    }
}

fn interval(rate: NonZeroU32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(rate.get()))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::path::PathBuf;

    use super::{FrameClock, ScriptGate};
    use crate::command::Command;

    #[test]
    fn changing_vsync_restarts_the_deadline() {
        let mut clock = FrameClock::new(NonZeroU32::new(60).unwrap());
        clock.set_rate(NonZeroU32::new(120).unwrap());
        assert_eq!(clock.rate().get(), 120);
        assert!(clock.time_until_tick() <= std::time::Duration::from_secs_f64(1.0 / 120.0));
    }

    #[test]
    fn frame_commands_wait_for_the_script_but_a_leading_quit_does_not() {
        let mut gate = ScriptGate::default();
        assert!(matches!(gate.accept(Command::Quit), Some(Command::Quit)));
        assert!(gate.accept(Command::Frame).is_none());
        assert!(gate.next_ready().is_none());

        gate.finish();
        assert!(matches!(gate.next_ready(), Some(Command::Frame)));
    }

    #[test]
    fn commands_after_a_deferred_screenshot_keep_their_input_order() {
        let mut gate = ScriptGate::default();
        let path = PathBuf::from("first-frame.png");
        assert!(
            gate.accept(Command::Screenshot(path.clone())).is_none(),
            "the first capture must wait for script completion"
        );
        assert!(gate.accept(Command::Quit).is_none());

        gate.finish();
        assert!(matches!(
            gate.next_ready(),
            Some(Command::Screenshot(actual)) if actual == path
        ));
        assert!(matches!(gate.next_ready(), Some(Command::Quit)));
    }
}
