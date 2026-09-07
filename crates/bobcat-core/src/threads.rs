//! What a thread of this engine is made of: how it is joined, and how it
//! reports having trapped.
//!
//! There are two: `bobcat-main`, which carries a group's view realms, and
//! `bobcat-workers`, which carries the same group's worker realms. Neither
//! knows about the other, and both end the same way.
//!
//! Waiting is deliberately not here. Both park on a
//! [`Mailbox`](crate::mailbox::Mailbox), whose timed receive is already
//! written once for both targets.

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
