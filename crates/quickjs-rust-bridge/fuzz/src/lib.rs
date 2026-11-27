//! What the fuzz targets share: the runtime every input gets, and the drain
//! that runs what the input queued.
//!
//! Each input runs on a fresh runtime, so a crash reproduces from the input
//! alone. Dropping the runtime is part of every run: `JS_FreeRuntime` asserts
//! that nothing it allocated is still reachable, so a reference the engine
//! or the bridge leaks aborts the input that leaked it.

use std::time::Duration;

use quickjs_rust_bridge::{Context, Runtime, RuntimeOptions};

const EXECUTION_TIMEOUT: Duration = Duration::from_millis(25);

const MEMORY_LIMIT: usize = 64 << 20;

#[must_use]
pub fn runtime() -> Option<Runtime> {
    Runtime::with_options(RuntimeOptions {
        memory_limit: Some(MEMORY_LIMIT),
        execution_timeout: Some(EXECUTION_TIMEOUT),
        ..RuntimeOptions::default()
    })
    .ok()
}

pub fn settle(runtime: &mut Runtime, realm: &Context) {
    for _ in 0..4 {
        if runtime.drain_pending_jobs(realm).is_ok() {
            return;
        }
        runtime.discard_unhandled_rejections(realm);
    }
}
