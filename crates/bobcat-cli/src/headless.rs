//! The headless embedder: a synthetic vsync clock and a command prompt over
//! the engine's offscreen output.
//!
//! The clock is this embedder's substitute for an OS display loop — it
//! relays ticks; whether a tick becomes GPU work is the engine's decision
//! (`tick` renders only when the document changed). Screenshots come back as
//! pixels, and writing the PNG is this side's IO.

use std::num::NonZeroU32;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use bobcat_core::engine::OffscreenEngine;

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
use crate::screenshot::save_screenshot;

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    let mut engine = OffscreenEngine::new(
        program.config,
        options.viewport_width,
        options.viewport_height,
        options.device_pixel_ratio,
    )?;
    if !program.author_css.is_empty() {
        engine.add_author_stylesheet(&program.author_css);
    }
    engine
        .run_script(&program.source)
        .map_err(|source| CliError::Script {
            input: program.input,
            source,
        })?;
    engine.attach_offscreen()?;

    engine.tick(true)?;
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
        let command = if running {
            match receiver.recv_timeout(clock.time_until_tick()) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => {
                    engine.tick(false)?;
                    clock.advance();
                    None
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => return Ok(()),
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
                engine.tick(true)?;
                clock.restart();
                println!("Rendered one frame.");
            }
            Command::Screenshot(path) => {
                // A screenshot failure must not tear down the session: report
                // it at the prompt like any other bad command and keep going.
                let result = engine
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
            // Do not render a burst of stale frames after a slow frame. Vsync
            // clocks expose the next opportunity, not a backlog of obligations.
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
