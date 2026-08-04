use std::num::NonZeroU32;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use bobcat_core::lynx_element::dom::render::gpu::Headless;
use bobcat_core::lynx_element::dom::vello::peniko::Color;

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::{FramePipeline, Program};
use crate::screenshot::save_screenshot;

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    // Headless drives its own synthetic clock, so it polls the frame slot at
    // each tick instead of being woken; the DOM thread needs no nudge.
    let mut pipeline = FramePipeline::start(
        program,
        options.viewport_width,
        options.viewport_height,
        options.device_pixel_ratio,
        || {},
    )?;
    let mut gpu = Headless::new().map_err(CliError::Gpu)?;

    render_frame(&mut pipeline, &mut gpu, true)?;
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
                    render_frame(&mut pipeline, &mut gpu, false)?;
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
                render_frame(&mut pipeline, &mut gpu, true)?;
                clock.restart();
                println!("Rendered one frame.");
            }
            Command::Screenshot(path) => {
                // A screenshot failure must not tear down the session: report
                // it at the prompt like any other bad command and keep going.
                if let Err(error) = capture(&mut pipeline, &mut gpu, &path) {
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

/// Renders one tick.
///
/// `force` means "the current document state, now": it takes the DOM
/// thread's barrier so the `frame` command and the first frame are
/// deterministic. An ordinary tick only paints what the DOM thread has
/// already published, so a slow script or layout pass costs skipped frames
/// rather than a stalled clock.
fn render_frame(
    pipeline: &mut FramePipeline,
    gpu: &mut Headless,
    force: bool,
) -> Result<(), CliError> {
    let frame = if force {
        pipeline.sync_frame()?
    } else {
        pipeline.request_frame();
        pipeline.prepare_frame()?
    };
    if !frame.changed && !force {
        // The retained target already holds this exact frame; re-submitting
        // it would burn a full GPU pass per tick on a static scene.
        return Ok(());
    }
    let scene = frame.scene;
    gpu.render_frame(scene, frame.size.width, frame.size.height, Color::WHITE)
        .map_err(CliError::Gpu)?;
    // Keep at most one frame in flight: nothing else in this loop
    // synchronizes with the GPU, so a clock that outpaces it would otherwise
    // pile up submissions without bound.
    gpu.wait_idle().map_err(CliError::Gpu)
}

fn capture(pipeline: &mut FramePipeline, gpu: &mut Headless, path: &Path) -> Result<(), CliError> {
    // A screenshot names the page as it is, so it waits for the DOM thread
    // rather than capturing whatever frame happens to be in the slot.
    let frame = pipeline.sync_frame()?;
    if frame.changed {
        let scene = frame.scene;
        gpu.render_frame(scene, frame.size.width, frame.size.height, Color::WHITE)
            .map_err(CliError::Gpu)?;
    }
    // The retained target holds the current frame; read it back rather than
    // re-rendering a scene that has not changed.
    let pixels = gpu.read_pixels().map_err(CliError::Gpu)?;
    save_screenshot(path, frame.size, &pixels)
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
