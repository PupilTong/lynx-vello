use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use lynx_element::dom::pulsar::gpu::Headless;
use lynx_element::dom::pulsar::vello::peniko::Color;

use super::pipeline::{CapturedFrame, RenderRuntime};
use super::{InputEvent, InputResponse, RenderError};

/// Windowless product renderer with an internal synthetic-vsync clock.
///
/// Normal ticks never read pixels back to the CPU and do not submit a GPU pass
/// for an unchanged document. [`Self::capture`] is the explicit readback seam.
pub struct HeadlessRenderer {
    runtime: RenderRuntime,
    gpu: Headless,
    clock: FrameClock,
    running: bool,
}

impl std::fmt::Debug for HeadlessRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeadlessRenderer")
            .field("runtime", &self.runtime)
            .field("vsync_hz", &self.clock.rate())
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

impl HeadlessRenderer {
    /// Creates the private GPU target and produces its initial frame.
    pub fn new(runtime: RenderRuntime, vsync_hz: NonZeroU32) -> Result<Self, RenderError> {
        let gpu = Headless::new().map_err(RenderError::from)?;
        let mut renderer = Self {
            runtime,
            gpu,
            clock: FrameClock::new(vsync_hz),
            running: true,
        };
        renderer.render(true)?;
        Ok(renderer)
    }

    #[must_use]
    pub const fn vsync_hz(&self) -> NonZeroU32 {
        self.clock.rate()
    }

    /// Changes the synthetic display rate and restarts its deadline.
    pub fn set_vsync_hz(&mut self, rate: NonZeroU32) {
        self.clock.set_rate(rate);
    }

    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    pub fn pause(&mut self) {
        self.running = false;
    }

    pub fn resume(&mut self) {
        self.running = true;
        self.clock.restart();
    }

    /// Time until the engine's next synthetic display opportunity.
    #[must_use]
    pub fn time_until_vsync(&self) -> Duration {
        self.clock.time_until_tick()
    }

    /// Advances one synthetic-vsync opportunity. Scene freshness and GPU
    /// submission are resolved internally.
    pub fn on_vsync(&mut self) -> Result<(), RenderError> {
        if !self.running {
            return Ok(());
        }
        self.render(false)?;
        self.clock.advance();
        Ok(())
    }

    /// Produces one frame even while paused and restarts the next deadline.
    pub fn render_one_frame(&mut self) -> Result<(), RenderError> {
        self.render(true)?;
        self.clock.restart();
        Ok(())
    }

    /// Routes one input event into the runtime. Any visual change is picked up
    /// by the next vsync or explicit capture.
    pub fn handle_input(&mut self, event: InputEvent) -> InputResponse {
        self.runtime.handle_input(event)
    }

    /// Captures the current rendered contents without exposing the retained
    /// scene or GPU texture.
    pub fn capture(&mut self) -> Result<CapturedFrame, RenderError> {
        let frame = self.runtime.prepare_frame();
        if frame.changed {
            let scene = frame.scene();
            self.gpu
                .render_frame(&scene, frame.size.width, frame.size.height, Color::WHITE)
                .map_err(RenderError::from)?;
        }
        let size = frame.size;
        drop(frame);
        let pixels = self.gpu.read_pixels().map_err(RenderError::from)?;
        self.clock.restart();
        Ok(CapturedFrame::new(size, pixels))
    }

    fn render(&mut self, force: bool) -> Result<(), RenderError> {
        let frame = self.runtime.prepare_frame();
        if !frame.changed && !force {
            return Ok(());
        }
        let scene = frame.scene();
        self.gpu
            .render_frame(&scene, frame.size.width, frame.size.height, Color::WHITE)
            .map_err(RenderError::from)?;
        drop(scene);
        drop(frame);
        // The paced engine owns its in-flight-work bound; embedders never poll
        // a wgpu device or submit a queue themselves.
        self.gpu.wait_idle().map_err(RenderError::from)
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
            // A display clock exposes the next opportunity, not a backlog of
            // catch-up frames after one slow render.
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
