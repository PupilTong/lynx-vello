//! What a worker realm is made of, on the group's *other* `QuickJS` runtime.
//!
//! Only that. Which realms exist, and when, is [`super::thread`]; the handle
//! the rest of the engine holds them by is [`super::Background`].

use std::cell::Cell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use crate::background::WorkerKey;
use crate::esm::{TIMER_MODULE_SPECIFIER, WORKER_MODULE_SPECIFIER};
use crate::link::{HostOutbox, ViewNotice};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

/// The worker realm's own host module. A worker realm declares two: this one,
/// with the members only a worker has, and `bobcat-internal:host`, which
/// carries the core [`crate::realm::open_realm`] installs in every realm and
/// none of the MTS realm's document members.
const WORKER_HOST_MODULE_SPECIFIER: &str = "bobcat-internal:worker";
/// Called on `bobcat:worker`, in a worker realm, with one message value.
pub(super) const WORKER_DELIVER_EXPORT: &str = "__BobcatDeliverWorkerMessage";
/// Called on `bobcat:worker` with one native module callback's answer.
pub(super) const WORKER_MODULE_CALLBACK_EXPORT: &str = "__BobcatNativeModuleCallback";

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

/// Installs a worker realm's own host module, `bobcat-internal:worker`: the
/// three members that are a worker's whole outward surface beyond the core
/// [`crate::realm::open_realm`] installed under `bobcat-internal:host`. The
/// BTS and a plain `Worker` get the same three. Answers with the flag
/// `closeWorker` sets.
///
/// There is no document member here and no way to add one: this realm is on
/// another runtime, on another thread, and the document is neither `Send` nor
/// reachable from anything the closures below capture. The native-module
/// member is no exception: it names a module and hands over text, and what
/// serves it is the embedder's own thread.
pub(super) fn install_worker_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    key: WorkerKey,
    host: &HostOutbox,
    mut post: impl FnMut(HostValue) + 'static,
) -> Result<Rc<Cell<bool>>, ScriptError> {
    install_native_modules(engine, js_runtime, key, host)?;

    engine.register_host_module_function(
        js_runtime,
        WORKER_HOST_MODULE_SPECIFIER,
        "postWorkerMessage",
        1,
        Box::new(move |arguments| {
            // Any value the boundary carries, which for anything that is not a
            // primitive is the structured clone the realm's own serializer
            // made. A value it refuses never arrives here at all: the
            // trampoline throws in the realm that called `postMessage`.
            post(arguments.first().cloned().unwrap_or(HostValue::Undefined));
            Ok(HostValue::Undefined)
        }),
    )?;

    let closing = Rc::new(Cell::new(false));
    let closer = Rc::clone(&closing);
    engine.register_host_module_function(
        js_runtime,
        WORKER_HOST_MODULE_SPECIFIER,
        "closeWorker",
        0,
        Box::new(move |_arguments| {
            // A flag, not a teardown: this runs inside the realm it would
            // tear down, so the thread reads it once the task returns.
            closer.set(true);
            Ok(HostValue::Undefined)
        }),
    )?;
    Ok(closing)
}

/// Installs the one member `NativeModules.<module>.<method>(...)` reaches the
/// embedder through.
///
/// Everything crosses as text, because everything here is JavaScript's: the
/// arguments are the realm's own JSON, and the function arguments are named by
/// the indices they occupied rather than carried. Nothing is built here but
/// the notice itself — the view assembles the call, because the handle a
/// callback answers through is the one the view already registered for this
/// worker. It rides the channel a source request uses, so a view that has
/// ended assembles nothing and the realm's functions are released.
fn install_native_modules(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    key: WorkerKey,
    host: &HostOutbox,
) -> Result<(), ScriptError> {
    const NAME: &str = "bobcat-internal:worker.invokeNativeModule";
    let host = host.clone();
    engine.register_host_module_function(
        js_runtime,
        WORKER_HOST_MODULE_SPECIFIER,
        "invokeNativeModule",
        5,
        Box::new(move |arguments| {
            let call = call_id(arguments)?;
            let module = string(arguments, 1)?.to_owned();
            let method = string(arguments, 2)?.to_owned();
            let call_arguments = string(arguments, 3)?.to_owned();
            let callbacks = string(arguments, 4)?
                .split(',')
                .filter(|index| !index.is_empty())
                .map(|index| {
                    index
                        .parse()
                        .map_err(|_| format!("{NAME} expects argument indices for argument 4"))
                })
                .collect::<Result<Vec<u32>, _>>()?;
            host.notify(ViewNotice::NativeModuleCall {
                worker: key,
                call,
                module,
                method,
                arguments: call_arguments,
                callbacks,
            });
            Ok(HostValue::Undefined)
        }),
    )
}

fn string(arguments: &[HostValue], index: usize) -> Result<&str, String> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value),
        _ => Err(format!(
            "bobcat-internal:worker.invokeNativeModule expects string argument {index}"
        )),
    }
}

/// The call number the realm minted, which it counts up from one and spells
/// as a number.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the checks below leave a whole, representable call number"
)]
fn call_id(arguments: &[HostValue]) -> Result<u64, String> {
    match arguments.first() {
        Some(&HostValue::Number(value))
            if value.is_finite() && value >= 0.0 && value.fract() == 0.0 =>
        {
            Ok(value as u64)
        }
        _ => Err(
            "bobcat-internal:worker.invokeNativeModule expects a call number for argument 0"
                .to_owned(),
        ),
    }
}
