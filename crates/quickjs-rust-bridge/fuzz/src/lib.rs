//! What the fuzz targets share: the runtime every input gets, and the drain
//! that runs what the input queued.
//!
//! Each input runs on a fresh runtime, so a crash reproduces from the input
//! alone. Dropping the runtime is part of every run: `JS_FreeRuntime` asserts
//! that nothing it allocated is still reachable, so a reference the engine
//! or the bridge leaks aborts the input that leaked it.

use std::time::Duration;

use quickjs_rust_bridge::{Context, Runtime, RuntimeOptions};

/// Long enough for any input libFuzzer is likely to keep, short enough that
/// a loop that never ends costs a few of these and not libFuzzer's timeout.
const EXECUTION_TIMEOUT: Duration = Duration::from_millis(25);

/// Keeps an input that allocates without bound to an out-of-memory
/// exception rather than libFuzzer's RSS limit.
const MEMORY_LIMIT: usize = 64 << 20;

/// A runtime under the limits every input runs with.
#[must_use]
pub fn runtime() -> Option<Runtime> {
    Runtime::with_options(RuntimeOptions {
        memory_limit: Some(MEMORY_LIMIT),
        execution_timeout: Some(EXECUTION_TIMEOUT),
        ..RuntimeOptions::default()
    })
    .ok()
}

/// Runs the jobs the input queued. A drain stops at the first job that
/// fails, and a job can queue another without end, so this drains a few
/// times and leaves the rest for the runtime to free.
pub fn settle(runtime: &mut Runtime, realm: &Context) {
    for _ in 0..4 {
        if runtime.drain_pending_jobs(realm).is_ok() {
            return;
        }
        runtime.discard_unhandled_rejections(realm);
    }
}
