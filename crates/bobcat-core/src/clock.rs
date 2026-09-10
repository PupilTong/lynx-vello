//! The one clock every realm's timers are armed against, and the one way to
//! wait out a deadline on it.

use std::future::Future;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use std::time::Instant as ClockInstant;

#[cfg(target_arch = "wasm32")]
pub(crate) use web_time::Instant as ClockInstant;

/// Resolves once `deadline` has passed.
///
/// Natively this is tokio's own time driver, running on the engine thread
/// that armed the timer. On wasm32 there is no time driver to enable —
/// tokio's reads `std::time::Instant`, which panics on that target — so the
/// same call is served by [`crate::alarm`], a thread that owns nothing but a
/// heap of deadlines and the wakers waiting on them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sleep_until(deadline: ClockInstant) -> impl Future<Output = ()> {
    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn sleep_until(deadline: ClockInstant) -> impl Future<Output = ()> {
    crate::alarm::sleep_until(deadline)
}
