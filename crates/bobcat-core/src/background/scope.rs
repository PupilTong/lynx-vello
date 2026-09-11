//! What a worker realm is made of, on the group's *other* `QuickJS` runtime.
//!
//! Only that. Which realms exist, and when, is [`super::thread`]; the handle
//! the rest of the engine holds them by is [`super::Background`].

use std::cell::Cell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use crate::esm::{
    BTS_RUNTIME_MODULE_SOURCE, BTS_RUNTIME_MODULE_SPECIFIER, CONTEXT_MODULE_SOURCE,
    CONTEXT_MODULE_SPECIFIER, EVENT_TARGET_MODULE_SPECIFIER, EVENT_TARGET_SOURCE,
    TIMER_MODULE_SOURCE, TIMER_MODULE_SPECIFIER,
};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;
use crate::timers::{TimerState, install_timer_members};

/// One string argument, or why it is not one.
///
/// A worker realm carries its own rather than borrowing the view realm's:
/// two members is the whole of its host surface, and reaching across for four
/// lines is what put the timers in the wrong module in the first place.
fn string_argument<'a>(
    function: &str,
    arguments: &'a [HostValue],
    index: usize,
) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}

/// The worker realm's global-scope module, on the *worker* runtime.
pub(super) const WORKER_MODULE_SPECIFIER: &str = "bobcat:worker";
/// The worker realm's host module: what `bobcat-internal:host` is to
/// `bobcat-main`, minus everything that would need a document.
const WORKER_HOST_MODULE_SPECIFIER: &str = "bobcat-internal:worker";
/// Called on `bobcat:worker`, in a worker realm, with one JSON message.
pub(super) const WORKER_DELIVER_EXPORT: &str = "__BobcatDeliverWorkerMessage";

const WORKER_MODULE_SOURCE: &str = crate::esm::runtime_source!("worker-runtime");

/// Registers the source modules every worker realm on the group's *worker*
/// runtime shares.
///
/// The worker runtime carries its global scope, timers, shared event machinery
/// and the BTS runtime module. Each script chooses its own imports.
/// `bobcat:element` and `bobcat:runtime` are absent because a worker has no
/// document to reach and no page to be the main thread of, and registering
/// them would make an import that must fail merely fail late.
pub(super) fn install_worker_modules(js_runtime: &mut ScriptRuntime) -> Result<(), ScriptError> {
    js_runtime.register_module_source(EVENT_TARGET_MODULE_SPECIFIER, EVENT_TARGET_SOURCE)?;
    js_runtime.register_module_source(WORKER_MODULE_SPECIFIER, WORKER_MODULE_SOURCE)?;
    js_runtime.register_module_source(CONTEXT_MODULE_SPECIFIER, CONTEXT_MODULE_SOURCE)?;
    js_runtime.register_module_source(BTS_RUNTIME_MODULE_SPECIFIER, BTS_RUNTIME_MODULE_SOURCE)?;
    js_runtime.register_module_source(TIMER_MODULE_SPECIFIER, TIMER_MODULE_SOURCE)
}

/// Every worker entry gets the same global scope, timers and name before its
/// own script. Any other bindings are installed by that script's imports.
///
/// The script is *inlined* rather than registered and imported, the same way
/// `ENTRY_PREAMBLE` carries the MTS entry. A module that is only evaluated
/// belongs to the realm that evaluated it and is never named on the runtime,
/// so two views that resolve one URL to different bytes cannot collide, and a
/// worker leaves no registration behind. The static imports run before
/// anything in the body, which is what puts the global scope and the timer
/// globals in place first; `name` is written between them and the script
/// because `self.name` is readable from a worker's top level.
///
/// The cost is the entry's cost: the preamble shifts the script's line
/// numbers by the lines above it. The module still carries the script's own
/// resolved URL, so a stack trace names the right file.
pub(super) fn worker_boot_source(name: &str, script: &str) -> String {
    let name = serde_json::to_string(name)
        .expect("serializing a Rust string as a JavaScript string cannot fail");
    format!(
        r#"import "{WORKER_MODULE_SPECIFIER}";
import "{TIMER_MODULE_SPECIFIER}";
globalThis.name = {name};
{script}"#
    )
}

/// Installs everything one worker realm reaches the host through: the timer
/// pair under `bobcat-internal:host`, so `bobcat:timers` compiles unchanged,
/// and the two members that are a worker's whole outward surface.
///
/// There is no document member here and no way to add one: this realm is on
/// another runtime, on another thread, and the document is neither `Send` nor
/// reachable from anything the closures below capture.
pub(super) fn install_worker_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    timers: &Rc<TimerState>,
    closing: &Rc<Cell<bool>>,
    mut post: impl FnMut(String) + 'static,
) -> Result<(), ScriptError> {
    install_timer_members(engine, js_runtime, timers)?;

    engine.register_host_module_function(
        js_runtime,
        WORKER_HOST_MODULE_SPECIFIER,
        "postWorkerMessage",
        1,
        Box::new(move |arguments| {
            const NAME: &str = "bobcat-internal:worker.postWorkerMessage";
            post(string_argument(NAME, arguments, 0)?.to_owned());
            Ok(HostValue::Undefined)
        }),
    )?;

    let closing = Rc::clone(closing);
    engine.register_host_module_function(
        js_runtime,
        WORKER_HOST_MODULE_SPECIFIER,
        "closeWorker",
        0,
        Box::new(move |_arguments| {
            // A flag, not a teardown: this runs inside the realm it would
            // tear down, so the thread reads it once the task returns.
            closing.set(true);
            Ok(HostValue::Undefined)
        }),
    )
}
