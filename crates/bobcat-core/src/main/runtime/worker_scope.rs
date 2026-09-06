//! What a worker realm is made of, on the group's *other* `QuickJS` runtime.
//!
//! The sibling [`worker_host`](super::worker_host) module is the mirror of
//! this one: it furnishes a *view's* realm with the `Worker` object a card
//! constructs, where this furnishes the realm that object stands for. They are
//! separate files because they run on separate runtimes on separate threads,
//! and nothing either builds can be named by the other.
//!
//! The thread itself, and the realms' lifecycle on it, is
//! [`crate::main::workers`].

use std::cell::Cell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use super::{
    EVENT_TARGET_MODULE_SPECIFIER, EVENT_TARGET_SOURCE, MainThreadError, ScriptEngine,
    ScriptRuntime, TIMER_MODULE_SOURCE, TIMER_MODULE_SPECIFIER, TimerState, install_timer_members,
    string_argument,
};
use crate::script::ScriptError;

/// The worker realm's global-scope module, on the *worker* runtime.
pub(crate) const WORKER_MODULE_SPECIFIER: &str = "bobcat:worker";
/// The module a worker realm's boot is evaluated as. It is never registered,
/// only evaluated, so every realm may carry one under the same name.
pub(crate) const WORKER_BOOT_SPECIFIER: &str = "bobcat:worker-boot";
/// The worker realm's host module: what `bobcat-internal:host` is to
/// `bobcat-main`, minus everything that would need a document.
const WORKER_HOST_MODULE_SPECIFIER: &str = "bobcat-internal:worker";
/// Called on `bobcat:worker`, in a worker realm, with one JSON message.
pub(crate) const WORKER_DELIVER_EXPORT: &str = "__BobcatDeliverWorkerMessage";

const WORKER_MODULE_SOURCE: &str =
    include_str!("../../../../../packages/bobcat-element/src/worker-runtime.mjs");

/// Registers the source modules every worker realm on the group's *worker*
/// runtime shares.
///
/// Deliberately short: the worker runtime carries the `EventTarget` its global
/// scope is built on, that global scope, and the timers — and nothing else.
/// `bobcat:element` and `bobcat:runtime` are absent because a worker has no
/// document to reach and no page to be the main thread of, and registering
/// them would make an import that must fail merely fail late.
pub(crate) fn install_worker_modules(js_runtime: &mut ScriptRuntime) -> Result<(), ScriptError> {
    js_runtime.register_module_source(EVENT_TARGET_MODULE_SPECIFIER, EVENT_TARGET_SOURCE)?;
    js_runtime.register_module_source(WORKER_MODULE_SPECIFIER, WORKER_MODULE_SOURCE)?;
    js_runtime.register_module_source(TIMER_MODULE_SPECIFIER, TIMER_MODULE_SOURCE)
}

/// The module one worker realm is booted with.
///
/// The two static imports run before anything in the body, which is what puts
/// the global scope and the timer globals in place before the worker's own
/// script is loaded — the same ordering `bobcat:boot` relies on for the MTS
/// entry. `name` is written between them and the script for the same reason:
/// `self.name` is readable from a worker's top level.
pub(crate) fn worker_boot_source(name: &str, url: &str) -> String {
    let name = serde_json::to_string(name)
        .expect("serializing a Rust string as a JavaScript string cannot fail");
    let url = serde_json::to_string(url)
        .expect("serializing a Rust string as a JavaScript string cannot fail");
    format!(
        r#"import "{WORKER_MODULE_SPECIFIER}";
import "{TIMER_MODULE_SPECIFIER}";

globalThis.name = {name};
await import({url});
"#
    )
}

/// Installs everything one worker realm reaches the host through: the timer
/// pair under `bobcat-internal:host`, so `bobcat:timers` compiles unchanged,
/// and the two members that are a worker's whole outward surface.
///
/// There is no document member here and no way to add one: this realm is on
/// another runtime, on another thread, and the document is neither `Send` nor
/// reachable from anything the closures below capture.
pub(crate) fn install_worker_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    timers: &Rc<TimerState>,
    closing: &Rc<Cell<bool>>,
    mut post: impl FnMut(String) + 'static,
) -> Result<(), ScriptError> {
    install_timer_members(engine, js_runtime, timers)
        .map_err(MainThreadError::into_script_error)?;

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
