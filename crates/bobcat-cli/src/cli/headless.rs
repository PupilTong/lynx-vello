//! The headless embedder: a synthetic vsync clock and a command prompt over
//! one offscreen painter.
//!
//! The clock is this embedder's substitute for an OS display loop — it
//! relays ticks; whether a tick becomes GPU work is the painter's decision
//! (a frame renders only when the document changed). Screenshots come back
//! as pixels, and writing the PNG is this side's IO. Both halves live on this
//! thread, the one that built them, so every one of those calls is the work
//! itself rather than a request for it.
//!
//! A frame is three calls with a wait in the middle ([`frame`]): the painter
//! posts it, this loop waits for the view to service it, the painter draws.
//! The wait is this loop's own because the view may need this thread during
//! it — a script parked on a stylesheet or a module is parked on an answer
//! `LynxView::pump` gives — so the wait pumps on every wakeup.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use bobcat_core::editing::{Command as EditCommand, TextInputState};
use bobcat_core::input::{InputEvent, KeyPhase, Modifiers, Point2D, PointerKind, PointerPhase};
use bobcat_core::{DrawTarget, EngineEvent, EventRequester, LynxView, Painter, ScreenMetrics};
use bobcat_resources::ViewResources;
use flume::RecvTimeoutError;

use crate::cli::CliError;
use crate::cli::args::Options;
use crate::cli::command::{COMMAND_HELP, Command, Console, describe_text_input};
use crate::cli::page::Program;
use crate::cli::screenshot::save_screenshot;

const FRAME_DEADLINE: Duration = Duration::from_secs(10);

const TAP_POINTER_ID: u32 = 1;

#[derive(Debug)]
struct ChannelWakeup(flume::Sender<HostEvent>);

impl EventRequester for ChannelWakeup {
    fn request_event(&self) {
        let _ = self.0.send(HostEvent::Pump);
    }
}

pub(crate) fn run(program: &Program, options: &Options) -> Result<(), CliError> {
    let (sender, receiver) = flume::unbounded();
    let event_requester = std::sync::Arc::new(ChannelWakeup(sender.clone()));
    let resources = program.resources({
        let requester = std::sync::Arc::clone(&event_requester);
        move || requester.request_event()
    });
    let group = super::start_group(
        event_requester,
        &options.locale,
        &options.preload_scripts,
        &program.input,
    )?;
    let (width, height, ratio) = (
        options.viewport_width,
        options.viewport_height,
        options.device_pixel_ratio,
    );
    let screen = ScreenMetrics::for_viewport(width, height, ratio);
    let sources = program.sources(screen, &options.locale);
    let mut view = group
        .create_lynx_view(
            width,
            height,
            ratio,
            resources.builder(),
            Vec::new(),
            sources,
        )
        .map_err(|source| CliError::StartView {
            input: program.input.clone(),
            source,
        })?;
    let mut painter =
        pollster::block_on(Painter::new(DrawTarget::Offscreen, width, height, ratio))?;
    painter.attach(&view)?;

    let mut script = ScriptGate::default();
    let mut held = VecDeque::new();
    let mut host = Host {
        view: &mut view,
        receiver: &receiver,
        held: &mut held,
        input: &program.input,
        text_input: None,
    };
    if frame(&mut host, &mut painter, true)? {
        script.finish();
    }
    let console = Console::start(move |command| sender.send(HostEvent::Command(command)).is_ok())
        .map_err(CliError::Console)?;
    let mut clock = FrameClock::new(options.vsync_hz);
    let mut running = true;
    println!(
        "bobcat: headless renderer running at {} Hz; enter `help` for commands",
        clock.rate().get()
    );
    console.prompt();

    loop {
        let command = if let Some(command) = script.next_ready() {
            Some(command)
        } else if let Some(command) = host.held.pop_front() {
            Some(command)
        } else if running {
            match receiver.recv_timeout(clock.time_until_tick()) {
                Ok(HostEvent::Command(command)) => Some(command),
                Ok(HostEvent::Pump) | Err(RecvTimeoutError::Timeout) => {
                    turn(&mut host, &mut painter, Some(&mut clock), &mut script)?;
                    None
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            match receiver.recv() {
                Ok(HostEvent::Command(command)) => Some(command),
                Ok(HostEvent::Pump) => {
                    turn(&mut host, &mut painter, None, &mut script)?;
                    None
                }
                Err(_) => return Ok(()),
            }
        };

        let Some(command) = command else {
            continue;
        };
        let Some(command) = script.accept(command) else {
            console.prompt();
            continue;
        };
        if execute_command(
            command,
            &mut host,
            &mut painter,
            &mut clock,
            &mut running,
            &mut script,
        )? {
            return Ok(());
        }
        console.prompt();
    }
}

fn execute_command(
    command: Command,
    host: &mut Host<'_>,
    painter: &mut Painter,
    clock: &mut FrameClock,
    running: &mut bool,
    script: &mut ScriptGate,
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
            if frame(host, painter, true)? {
                script.finish();
            }
            clock.restart();
            println!("Rendered one frame.");
        }
        Command::Screenshot(path) => {
            if await_pictures(host, painter)? {
                script.finish();
            }
            let result = painter
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
        Command::Tap(x, y) => {
            for phase in [PointerPhase::Down, PointerPhase::Up] {
                painter.dispatch_input(InputEvent::pointer(
                    Point2D::new(x, y),
                    TAP_POINTER_ID,
                    PointerKind::Touch,
                    phase,
                ));
            }
        }
        Command::Key { key, text } => {
            let key = std::sync::Arc::<str>::from(key);
            for (phase, text) in [(KeyPhase::Down, text), (KeyPhase::Up, None)] {
                painter.dispatch_input(
                    InputEvent::key(phase, key.clone(), "", false, Modifiers::default())
                        .with_text(text.map(Into::into)),
                );
            }
        }
        Command::Compose { text, selection } => {
            edit(host.view, EditCommand::SetComposition { text, selection });
        }
        Command::Commit(text) => edit(
            host.view,
            EditCommand::CommitText {
                text,
                relative_cursor: 0,
            },
        ),
        Command::ShowTextInput => {
            if check_script(host)? {
                script.finish();
            }
            println!("{}", describe_text_input(host.text_input.as_ref()));
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

struct Host<'a> {
    view: &'a mut LynxView<ViewResources>,
    receiver: &'a flume::Receiver<HostEvent>,
    held: &'a mut VecDeque<Command>,
    input: &'a str,
    text_input: Option<TextInputState>,
}

fn edit(view: &LynxView<ViewResources>, command: EditCommand) {
    if let Err(error) = view.edit(command) {
        eprintln!("bobcat: the view refused the edit: {error}");
    }
}

fn turn(
    host: &mut Host<'_>,
    painter: &mut Painter,
    clock: Option<&mut FrameClock>,
    script: &mut ScriptGate,
) -> Result<(), CliError> {
    if check_script(host)? {
        script.finish();
    }
    if let Some(clock) = clock
        && clock.time_until_tick().is_zero()
    {
        if frame(host, painter, false)? {
            script.finish();
        }
        clock.advance();
    }
    Ok(())
}

fn frame(host: &mut Host<'_>, painter: &mut Painter, force: bool) -> Result<bool, CliError> {
    let posted = painter.post_frame()?;
    let deadline = Instant::now() + FRAME_DEADLINE;
    let mut finished = false;
    loop {
        finished |= check_script(host)?;
        if painter.frame_serviced(&posted) {
            break;
        }
        match host.receiver.recv_deadline(deadline) {
            Ok(HostEvent::Pump) => {}
            Ok(HostEvent::Command(command)) => host.held.push_back(command),
            Err(_) => break,
        }
    }
    painter.draw_posted(posted, force)?;
    Ok(finished)
}

fn await_pictures(host: &mut Host<'_>, painter: &mut Painter) -> Result<bool, CliError> {
    let deadline = Instant::now() + FRAME_DEADLINE;
    let mut finished = false;
    loop {
        finished |= check_script(host)?;
        if !painter.awaits_pictures() {
            return Ok(finished);
        }
        match host.receiver.recv_deadline(deadline) {
            Ok(HostEvent::Pump) => {}
            Ok(HostEvent::Command(command)) => host.held.push_back(command),
            Err(_) => return Ok(finished),
        }
    }
}

fn check_script(host: &mut Host<'_>) -> Result<bool, CliError> {
    let mut finished = false;
    for event in host.view.pump() {
        match event {
            event if event.is_fatal() => return Err(CliError::from_fatal_event(host.input, event)),
            EngineEvent::ScriptFinished => finished = true,
            EngineEvent::TextInputStateChanged(state) => host.text_input = Some(*state),
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
            _ => {}
        }
    }
    Ok(finished)
}

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
    matches!(
        command,
        Command::Frame
            | Command::Screenshot(_)
            | Command::Tap(..)
            | Command::Key { .. }
            | Command::Compose { .. }
            | Command::Commit(_)
            | Command::ShowTextInput
    )
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
    use crate::cli::command::Command;

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
