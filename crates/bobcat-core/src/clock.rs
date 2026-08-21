//! The animation timeline.
//!
//! A view is generic over its clock and names one at construction, so every
//! reading is a direct call — a timeline is a type, not a trait object.
//! [`SystemClock`] is the default and reads the platform's monotonic clock, so
//! `LynxView::new` needs nothing from the host.
//!
//! Two hosts name their own. A browser has a better reading than it could take
//! itself: `requestAnimationFrame` hands over the frame's timestamp, the
//! instant the frame is *for*, where reading a clock partway through producing
//! the frame drifts and jitters — browsers standardised on the former for
//! exactly that reason. And a test or a scripted capture wants [`ManualClock`],
//! so the same script plus the same sample times give the same pixels on every
//! machine. Both drive the clock from outside the view, which is what the
//! [`AnimationClock`] implementation on `Arc` is for.
//!
//! Whichever is installed, the engine samples it once per frame on the
//! presenting side and hands that one value to the document, so every animation
//! in a frame is sampled at the same instant.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(target_arch = "wasm32")]
use web_time::Instant;

/// An animation timeline, in seconds on a monotonic timescale whose epoch is
/// the clock's own choice.
///
/// Same contract as Stylo's `current_time_for_animations`, which is what
/// `animation-duration` and `animation-delay` are measured against. The engine
/// reads it on the presenting thread, once per frame.
pub trait AnimationClock: Send + Sync + 'static {
    fn now_seconds(&self) -> f64;
}

/// A shared clock is a clock, so a host can keep a handle to the one its view
/// reads and write the frame's time into it. This is the whole reason the
/// timeline needs no runtime replacement: the type is fixed at construction
/// and the *reading* is what moves.
impl<T: AnimationClock + ?Sized> AnimationClock for Arc<T> {
    fn now_seconds(&self) -> f64 {
        (**self).now_seconds()
    }
}

/// The platform's monotonic clock — the timeline a view uses unless its host
/// installs another.
///
/// `std::time::Instant` natively and `web_time::Instant` on Wasm, the same
/// split `quickjs-rust-bridge` already uses for its own timings. The epoch is
/// when the clock was made, which is view construction, so the numbers stay
/// small and readable.
#[derive(Debug)]
pub struct SystemClock {
    epoch: Instant,
}

impl SystemClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimationClock for SystemClock {
    fn now_seconds(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
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
    use super::{AnimationClock, ManualClock, SystemClock};

    #[test]
    fn the_default_clock_starts_near_zero_and_never_goes_back() {
        let clock = SystemClock::new();
        let first = clock.now_seconds();
        let second = clock.now_seconds();
        assert!((0.0..1.0).contains(&first), "the epoch is clock creation");
        assert!(second >= first, "and time only moves forward");
    }

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
