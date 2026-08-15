//! The headless embedder: a synthetic vsync clock and a command prompt over
//! the view's offscreen output.
//!
//! The clock is this embedder's substitute for an OS display loop — it
//! relays ticks; whether a tick becomes GPU work is the view's decision
//! (`tick` renders only when the document changed). Screenshots come back as
//! pixels, and writing the PNG is this side's IO.

use std::num::NonZeroU32;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use bobcat_core::{EngineEvent, OffscreenLynxView, quickjs_engine_factory};

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
use crate::screenshot::save_screenshot;

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    program.warn_about_dropped_author_rules();
    let mut view = OffscreenLynxView::new(
        program.config,
        program.resource_fetcher,
        quickjs_engine_factory(),
        options.viewport_width,
        options.viewport_height,
        options.device_pixel_ratio,
    )?;
    view.attach_offscreen()?;
    pollster::block_on(view.execute_script(program.script_url.as_str())).map_err(|source| {
        CliError::StartScript {
            input: program.input.clone(),
            source,
        }
    })?;

    view.tick(true)?;
    let (sender, receiver) = mpsc::channel();
    let console =
        Console::start(move |command| sender.send(command).is_ok()).map_err(CliError::Console)?;
    let mut clock = FrameClock::new(options.vsync_hz);
    let mut running = true;
    println!(
        "bobcat: headless renderer running at {} Hz; enter `help` for commands",
        clock.rate().get()
    );
    console.prompt();

    loop {
        check_script(&mut view, &program.input)?;
        let command = if running {
            match receiver.recv_timeout(clock.time_until_tick()) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => {
                    view.tick(false)?;
                    clock.advance();
                    None
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        };

        let Some(command) = command else {
            continue;
        };
        match command {
            Command::Continue => {
                running = true;
                clock.restart();
                println!("Continuing at {} Hz.", clock.rate().get());
            }
            Command::Pause => {
                running = false;
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
            Command::Quit => return Ok(()),
            Command::Invalid(message) => eprintln!("bobcat: {message}"),
        }
        console.prompt();
    }
}

fn check_script(view: &mut OffscreenLynxView, input: &str) -> Result<(), CliError> {
    let error = view.pump().into_iter().find_map(|event| match event {
        EngineEvent::ScriptFinished(Err(source)) => Some(source),
        _ => None,
    });
    match error {
        Some(source) => Err(CliError::Script {
            input: input.to_owned(),
            source,
        }),
        None => Ok(()),
    }
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

    use super::FrameClock;

    #[test]
    fn changing_vsync_restarts_the_deadline() {
        let mut clock = FrameClock::new(NonZeroU32::new(60).unwrap());
        clock.set_rate(NonZeroU32::new(120).unwrap());
        assert_eq!(clock.rate().get(), 120);
        assert!(clock.time_until_tick() <= std::time::Duration::from_secs_f64(1.0 / 120.0));
    }
}
