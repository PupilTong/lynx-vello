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

use bobcat_core::{EngineEvent, EventRequester, OffscreenLynxView, quickjs_engine_factory};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
use crate::screenshot::save_screenshot;

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    program.warn_about_compatibility_limits();
    let (sender, receiver) = mpsc::channel();
    let event_sender = sender.clone();
    let event_requester: std::sync::Arc<dyn EventRequester> = std::sync::Arc::new(move || {
        let _ = event_sender.send(HostEvent::Pump);
    });
    let style_sheet_url = program.resource_fetcher.style_sheet_url().cloned();
    let mut view = OffscreenLynxView::new(
        program.config,
        program.resource_fetcher,
        quickjs_engine_factory(),
        event_requester,
        options.viewport_width,
        options.viewport_height,
        options.device_pixel_ratio,
    )?;
    view.attach_offscreen()?;
    // Author CSS mounts before the script builds its tree, so the first
    // commit is already styled.
    if let Some(url) = style_sheet_url.as_ref() {
        pollster::block_on(view.load_style_sheet(url.as_str())).map_err(|source| {
            CliError::LoadStyleSheet {
                input: program.input.clone(),
                source,
            }
        })?;
    }
    pollster::block_on(view.execute_script(program.script_url.as_str())).map_err(|source| {
        CliError::StartScript {
            input: program.input.clone(),
            source,
        }
    })?;

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
                    if !script.is_finished() && check_script(&mut view, &program.input)? {
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
                    if !script.is_finished() && check_script(&mut view, &program.input)? {
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
    view: &mut OffscreenLynxView,
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

fn check_script(view: &mut OffscreenLynxView, input: &str) -> Result<bool, CliError> {
    let mut finished = false;
    for event in view.pump() {
        match event {
            EngineEvent::ScriptFinished(Ok(())) => finished = true,
            EngineEvent::ScriptFinished(Err(source)) => {
                return Err(CliError::Script {
                    input: input.to_owned(),
                    source,
                });
            }
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
    const fn is_finished(&self) -> bool {
        self.finished
    }

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
