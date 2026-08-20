//! The animation timeline capability.
//!
//! Bobcat interpolates animations but does not read a clock: a native host
//! has `std::time`, a browser Worker has `requestAnimationFrame`'s timestamp,
//! and a test wants neither. The host supplies the reading; the engine samples
//! it once per frame on the presenting side and hands that one value to the
//! document, so every animation in a frame is sampled at the same instant.
//!
//! A view with no clock installed never animates. There is no wall-clock
//! fallback: OS time is an embedder capability like input and the draw target
//! (see `docs/runtime-architecture.md`), and a silent fallback would make the
//! timeline untestable and unavailable on Wasm anyway.

use std::sync::atomic::{AtomicU64, Ordering};

/// The host's animation timeline, in seconds on a monotonic timescale whose
/// epoch is the host's choice.
///
/// Same contract as Stylo's `current_time_for_animations`, which is what
/// `animation-duration` and `animation-delay` are measured against. The engine
/// reads it on the presenting thread, once per frame.
pub trait AnimationClock: Send + Sync + 'static {
    fn now_seconds(&self) -> f64;
}

/// A clock the host steps by hand, so a frame sequence is reproducible.
///
/// Tests and deterministic offscreen capture use this instead of real time:
/// the same script plus the same sample times give the same pixels on every
/// machine.
#[derive(Debug, Default)]
pub struct ManualClock {
    /// The current reading, as `f64` bits. Stored bitwise so the value that
    /// crosses threads is the exact one the host set, with no scaling.
    seconds: AtomicU64,
}

impl ManualClock {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            seconds: AtomicU64::new(0),
        }
    }

    /// Moves the timeline to `seconds`. Values below zero, and NaN, pin to
    /// zero: a timeline that runs backwards is not one Stylo can sample.
    pub fn set(&self, seconds: f64) {
        self.seconds
            .store(seconds.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Moves the timeline forward by `seconds`.
    pub fn advance(&self, seconds: f64) {
        self.set(self.now_seconds() + seconds);
    }
}

impl AnimationClock for ManualClock {
    fn now_seconds(&self) -> f64 {
        f64::from_bits(self.seconds.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::{AnimationClock, ManualClock};

    #[test]
    fn a_manual_clock_starts_at_zero_and_steps_exactly() {
        let clock = ManualClock::new();
        assert!((clock.now_seconds() - 0.0).abs() < f64::EPSILON);
        clock.set(1.5);
        assert!((clock.now_seconds() - 1.5).abs() < 1e-9);
        clock.advance(0.25);
        assert!((clock.now_seconds() - 1.75).abs() < 1e-9);
    }

    #[test]
    fn a_manual_clock_never_reports_negative_time() {
        let clock = ManualClock::new();
        clock.set(-5.0);
        assert!((clock.now_seconds() - 0.0).abs() < f64::EPSILON);
    }
}
