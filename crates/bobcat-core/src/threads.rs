//! What a thread of this engine is made of: how it is joined, and how it
//! reports having trapped.
//!
//! There are two: `bobcat-main`, which carries a group's view realms, and
//! `bobcat-workers`, which carries the same group's worker realms. Neither
//! knows about the other, and both end the same way.
//!
//! Waiting is deliberately not here. Each runs a tokio runtime of its own and
//! parks in its scheduler, so what is left in common is only how a thread is
//! joined and how one reports having trapped.

use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};

/// The right to wait for one of this engine's threads.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type JoinHandle = std::thread::JoinHandle<()>;
#[cfg(target_arch = "wasm32")]
pub(crate) type JoinHandle = wasm_thread::JoinHandle<()>;

/// Waits for a thread that has already been told to end.
///
/// Under `panic = "abort"` a trapped wasm thread runs no destructors and
/// never signals its join handle, and no check can outrun a trap that lands
/// between the check and the wait — so wasm teardown never joins. The goodbye
/// is already sent: a healthy thread exits on its own, and a trapped one is
/// already gone.
pub(crate) fn join(thread: JoinHandle) {
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
