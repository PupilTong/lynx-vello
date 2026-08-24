//! The animation timeline.
//!
//! The engine owns it. There is no trait, no injection point, and no way for
//! an embedder to hand one in: a view reads the platform's monotonic clock,
//! `std::time::Instant` natively and `web_time::Instant` on Wasm, and that is
//! the whole contract. The epoch is view construction, so the numbers stay
//! small and readable.
//!
//! A host has nothing better to offer. Presentation runs on
//! `PresentMode::AutoVsync`, so the swap chain — not the embedder — is what
//! paces frames, and the engine samples the clock *after* the acquire that
//! waits on it, at the point where the frame it is about to produce is the
//! next one to reach the display. A browser's `requestAnimationFrame`
//! timestamp cannot improve on that: it is taken on the page's main thread,
//! before the Render Worker is even woken, and on a different time origin
//! than the Worker's own `performance.now()`.
//!
//! The engine samples once per frame on the presenting side and passes that
//! one reading to everything the frame resolves — gesture deadlines and
//! animations alike — so a frame has exactly one instant.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(target_arch = "wasm32")]
use web_time::Instant;

/// The engine's animation timeline, in seconds on a monotonic timescale whose
/// epoch is clock creation.
///
/// Same contract as Stylo's `current_time_for_animations`, which is what
/// `animation-duration` and `animation-delay` are measured against.
#[derive(Debug)]
pub(crate) struct FrameClock {
    epoch: Instant,
    /// A pinned reading, so a test's frame sequence is reproducible: the same
    /// script plus the same sample times give the same pixels on every
    /// machine. The field does not exist in a non-test build — the timeline
    /// an embedder gets is real time and nothing else.
    #[cfg(test)]
    pinned: Option<f64>,
}

impl FrameClock {
    pub(crate) fn new() -> Self {
        Self {
            epoch: Instant::now(),
            #[cfg(test)]
            pinned: None,
        }
    }

    /// The current reading. Called once per frame, on the presenting thread.
    pub(crate) fn now_seconds(&self) -> f64 {
        #[cfg(test)]
        if let Some(seconds) = self.pinned {
            return seconds;
        }
        self.epoch.elapsed().as_secs_f64()
    }

    /// Pins the timeline to `seconds` until the next call. Values below zero,
    /// and NaN, pin to zero: a timeline that runs backwards is not one Stylo
    /// can sample.
    #[cfg(test)]
    pub(crate) fn pin(&mut self, seconds: f64) {
        self.pinned = Some(seconds.max(0.0));
    }
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::FrameClock;

    #[test]
    fn the_clock_starts_near_zero_and_never_goes_back() {
        let clock = FrameClock::new();
        let first = clock.now_seconds();
        let second = clock.now_seconds();
        assert!((0.0..1.0).contains(&first), "the epoch is clock creation");
        assert!(second >= first, "and time only moves forward");
    }

    #[test]
    fn a_pinned_clock_holds_the_instant_a_test_named() {
        let mut clock = FrameClock::new();
        clock.pin(1.5);
        assert!((clock.now_seconds() - 1.5).abs() < 1e-9);
        clock.pin(1.75);
        assert!((clock.now_seconds() - 1.75).abs() < 1e-9);
    }

    #[test]
    fn a_pinned_clock_never_reports_negative_time() {
        let mut clock = FrameClock::new();
        clock.pin(-5.0);
        assert!((clock.now_seconds() - 0.0).abs() < f64::EPSILON);
    }
}
