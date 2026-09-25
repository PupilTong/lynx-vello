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

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::sync::OnceLock;

use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
static WASM_SCRIPT_PANIC_HOOK: OnceLock<()> = OnceLock::new();

/// Reports a panic on the thread that installed it, over whatever link that
/// thread holds. Erased to a closure because a `thread_local!` static cannot
/// be generic — and the hook it feeds is process-global anyway.
///
/// It is handed what the panic said, without a subject: each thread's
/// reporter names itself, so one hook serves both of them.
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
pub(crate) type ScriptPanicReporter = Box<dyn Fn(&str)>;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
thread_local! {
    /// On `bobcat-main`, one reporter per view that thread has ever carried;
    /// on `bobcat-workers`, the one reporter for every worker on it.
    /// Append-only: a view that is gone has a closed channel, and sending onto
    /// one is already a no-op, so nothing has to be pruned on a path that only
    /// runs as the thread traps.
    static WASM_SCRIPT_PANIC_REPORTERS: RefCell<Vec<ScriptPanicReporter>> = const {
        RefCell::new(Vec::new())
    };
}

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

/// Installs the process-wide hook that hands a panic to the reporters of the
/// thread it happened on, once for both threads.
///
/// Under `panic = "abort"` nothing unwinds, so no task's owner ever sees the
/// panic: the hook, which runs before the abort, is the only place left to
/// report it from. Each thread calls this before it registers a reporter,
/// because either may be the first to start — `bobcat-workers` is started
/// before `bobcat-main`.
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
pub(crate) fn install_script_panic_hook() {
    WASM_SCRIPT_PANIC_HOOK.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            WASM_SCRIPT_PANIC_REPORTERS.with(|reporters| {
                let location = info
                    .location()
                    .map_or_else(String::new, |location| format!(" at {location}"));
                let detail = format!(
                    "aborted after a panic{location}: {}",
                    panic_message(info.payload())
                );
                for reporter in reporters.borrow().iter() {
                    reporter(&detail);
                }
            });
            previous(info);
        }));
    });
}

/// Adds one reporter for panics on the calling thread.
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
pub(crate) fn add_script_panic_reporter(reporter: ScriptPanicReporter) {
    WASM_SCRIPT_PANIC_REPORTERS.with(|reporters| reporters.borrow_mut().push(reporter));
}
