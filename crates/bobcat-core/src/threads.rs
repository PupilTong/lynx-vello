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

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
pub(crate) type ScriptPanicReporter = Box<dyn Fn(&str)>;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
thread_local! {
    static WASM_SCRIPT_PANIC_REPORTERS: RefCell<Vec<ScriptPanicReporter>> = const {
        RefCell::new(Vec::new())
    };
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type JoinHandle = std::thread::JoinHandle<()>;
#[cfg(target_arch = "wasm32")]
pub(crate) type JoinHandle = wasm_thread::JoinHandle<()>;

pub(crate) struct ThreadJoin(Option<JoinHandle>);

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

fn join(thread: JoinHandle) {
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    drop(thread);
    #[cfg(not(all(target_arch = "wasm32", panic = "abort")))]
    {
        let _ = thread.join();
    }
}

pub(crate) fn platform_script_error(message: String) -> ScriptError {
    ScriptError {
        kind: ScriptErrorKind::Other,
        phase: ScriptErrorPhase::Execute,
        message: std::sync::Arc::from(message),
        location: None,
    }
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else {
        "non-string panic payload"
    }
}

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

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
pub(crate) fn add_script_panic_reporter(reporter: ScriptPanicReporter) {
    WASM_SCRIPT_PANIC_REPORTERS.with(|reporters| reporters.borrow_mut().push(reporter));
}
