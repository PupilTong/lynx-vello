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
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::SystemTime;

pub(crate) use terminus::link::clock::ClockInstant;
#[cfg(target_arch = "wasm32")]
pub use web_time::SystemTime;

pub(crate) const ALIGNMENT_PATIENCE: Duration = Duration::from_secs(2);

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sleep_until(deadline: ClockInstant) -> impl Future<Output = ()> {
    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.into_std()))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn sleep_until(deadline: ClockInstant) -> impl Future<Output = ()> {
    crate::alarm::sleep_until(deadline)
}
