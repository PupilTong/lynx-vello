//! What a thread of this engine is made of: how it is joined, and how it
//! reports having trapped.
//!
//! There are two: `bobcat-main`, which carries a group's view realms, and
//! `bobcat-workers`, which carries the same group's worker realms. Neither
//! knows about the other, and both end the same way.
//!
//! Parking is deliberately not here. Each runs a tokio runtime of its own and
//! parks in its scheduler, so what is left in common is only [`ThreadJoin`] —
//! how a thread is waited for — and how one reports having trapped.

use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};

/// The right to wait for one of this engine's threads.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type JoinHandle = std::thread::JoinHandle<()>;
#[cfg(target_arch = "wasm32")]
pub(crate) type JoinHandle = wasm_thread::JoinHandle<()>;

/// One of this engine's threads, waited for by this value's own drop.
///
/// Held after whatever closes that thread's inbox, so the wait is reached with
/// the goodbye already said: a group's field order is the whole of its
/// teardown, and nothing has to call a join by hand.
pub(crate) struct ThreadJoin(
    /// `Option` only because [`Drop`] cannot move out of `&mut self`. It is
    /// `Some` for the whole of this value's life.
    Option<JoinHandle>,
);

impl ThreadJoin {
    pub(crate) const fn new(thread: JoinHandle) -> Self {
        Self(Some(thread))
    }
}

impl Drop for ThreadJoin {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            join(thread);
        }
    }
}

/// Waits for a thread that has already been told to end.
///
/// Under `panic = "abort"` a trapped wasm thread runs no destructors and
/// never signals its join handle, and no check can outrun a trap that lands
/// between the check and the wait — so wasm teardown never joins. The goodbye
/// is already sent: a healthy thread exits on its own, and a trapped one is
/// already gone.
fn join(thread: JoinHandle) {
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    drop(thread);
    #[cfg(not(all(target_arch = "wasm32", panic = "abort")))]
    {
        let _ = thread.join();
    }
}

/// A failure of the host rather than of any script: a thread that trapped, or
/// one that would not start.
pub(crate) fn platform_script_error(message: String) -> ScriptError {
    ScriptError {
        kind: ScriptErrorKind::Other,
        phase: ScriptErrorPhase::Execute,
        message: std::sync::Arc::from(message),
        location: None,
    }
}

/// One panic, worded once, for every place that reports one on.
///
/// `prefix` names the thread it happened on — a view's tasks and a worker's
/// tasks are the only two that report a panic as a script failure — and the
/// wording is here rather than at each site so a panic a view's owner reaps
/// reads the same as one its group's thread reaps.
pub(crate) fn panicked(prefix: &'static str, payload: &(dyn std::any::Any + Send)) -> ScriptError {
    platform_script_error(format!("{prefix}: {}", panic_message(payload)))
}

/// What a panic said, for the two threads that catch one and report it on.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else {
        "non-string panic payload"
    }
}
