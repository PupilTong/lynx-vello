//! The input as the text of a classic script: parsed, compiled and run in a
//! fresh realm, then every job it queued.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quickjs_rust_bridge::{EvalOptions, EvalSource};
use quickjs_rust_bridge_fuzz::{runtime, settle};

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let Some(mut runtime) = runtime() else {
        return;
    };
    let Ok(realm) = runtime.create_context() else {
        return;
    };
    // Errors are the input's business; only a crash, a sanitizer report or a
    // leak `JS_FreeRuntime` finds is a finding.
    let _ = realm.evaluate(EvalSource::new(&source), EvalOptions::default());
    settle(&mut runtime, &realm);
    drop(realm);
    drop(runtime);
});
