//! The one clock every realm's timers are armed against, and the one way to
//! wait out a deadline on it; and the wall clock a view reads once, at its
//! construction, to place that monotonic clock on the Unix timeline
//! ([`crate::performance::TimeOrigin`]).
//!
//! The monotonic clock is `terminus`'s
//! ([`terminus::link::clock::ClockInstant`]), so an instant `terminus` takes
//! — a commit's timing marks, a presented frame's — and one this crate takes
//! are the same type and subtract. On wasm32 each engine thread is aligned to
//! the thread that spawned it as its first statement, so every engine
//! thread reads the embedder thread's clock: `bobcat-main` and
//! `bobcat-workers` by [`crate::LynxGroup::new`], the alarm thread by
//! whichever engine thread starts it ([`crate::alarm`]).

use std::future::Future;
use std::time::Duration;
/// The wall clock: read once per view, beside a [`ClockInstant`], and the
/// type the embedder's [`crate::performance::ExtraTiming`] marks arrive in.
/// `std::time::SystemTime` panics on `wasm32-unknown-unknown` like `Instant`
/// does, so that target reads `web_time`'s.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use std::time::SystemTime;

pub(crate) use terminus::link::clock::ClockInstant;
#[cfg(target_arch = "wasm32")]
pub(crate) use web_time::SystemTime;

/// How long a spawner waits for a thread it spawned to align its clock
/// before letting it run on its own. Only reached by a thread that does not
/// start: the exchange itself is one round trip between two threads.
pub(crate) const ALIGNMENT_PATIENCE: Duration = Duration::from_secs(2);

/// Resolves once `deadline` has passed.
///
/// Natively this is tokio's own time driver, running on the engine thread
/// that armed the timer. On wasm32 there is no time driver to enable —
/// tokio's reads `std::time::Instant`, which panics on that target — so the
/// same call is served by [`crate::alarm`], a thread that owns nothing but a
/// heap of deadlines and the wakers waiting on them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sleep_until(deadline: ClockInstant) -> impl Future<Output = ()> {
    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.into_std()))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn sleep_until(deadline: ClockInstant) -> impl Future<Output = ()> {
    crate::alarm::sleep_until(deadline)
}
