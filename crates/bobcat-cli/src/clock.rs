//! The native animation timeline.
//!
//! OS time is embedder work, like input and the draw target: `bobcat-core`
//! interpolates animations but reads no clock of its own. This is the CLI's
//! reading of one, shared by the windowed and headless runners.

use std::time::Instant;

use bobcat_core::AnimationClock;

/// Seconds since the view was created, from the platform's monotonic clock.
///
/// The epoch is arbitrary — Stylo compares `animation-delay` and
/// `animation-duration` against differences, never against the value itself —
/// so starting at view creation keeps the numbers small and readable in logs.
#[derive(Debug)]
pub(crate) struct MonotonicClock {
    epoch: Instant,
}

impl MonotonicClock {
    pub(crate) fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl AnimationClock for MonotonicClock {
    fn now_seconds(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use bobcat_core::AnimationClock;

    use super::MonotonicClock;

    #[test]
    fn a_monotonic_clock_starts_near_zero_and_never_goes_back() {
        let clock = MonotonicClock::new();
        let first = clock.now_seconds();
        let second = clock.now_seconds();
        assert!((0.0..1.0).contains(&first), "the epoch is view creation");
        assert!(second >= first, "and time only moves forward");
    }
}
