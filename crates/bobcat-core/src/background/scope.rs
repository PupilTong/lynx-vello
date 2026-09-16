//! What a worker realm is made of, on the group's *other* `QuickJS` runtime.
//!
//! Only that. Which realms exist, and when, is [`super::thread`]; the handle
//! the rest of the engine holds them by is [`super::Background`].

use std::cell::Cell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
use tokio::sync::mpsc;

use crate::background::WorkerMessage;
use crate::esm::{
    BTS_RUNTIME_MODULE_SOURCE, BTS_RUNTIME_MODULE_SPECIFIER, CONTEXT_MODULE_SOURCE,
    CONTEXT_MODULE_SPECIFIER, EVENT_TARGET_MODULE_SPECIFIER, EVENT_TARGET_SOURCE,
    GLOBAL_EVENT_MODULE_SOURCE, GLOBAL_EVENT_MODULE_SPECIFIER, TIMER_MODULE_SOURCE,
    TIMER_MODULE_SPECIFIER,
};
use crate::link::{HostOutbox, ViewNotice};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::native_module::{ModuleCall, ModuleCallback};
use crate::script::ScriptError;
use crate::timers::{TimerState, install_timer_members};

/// The worker realm's global-scope module, on the *worker* runtime.
pub(super) const WORKER_MODULE_SPECIFIER: &str = "bobcat:worker";
/// The worker realm's host module: what `bobcat-internal:host` is to
/// `bobcat-main`, minus everything that would need a document.
const WORKER_HOST_MODULE_SPECIFIER: &str = "bobcat-internal:worker";
/// Called on `bobcat:worker`, in a worker realm, with one message value.
pub(super) const WORKER_DELIVER_EXPORT: &str = "__BobcatDeliverWorkerMessage";
/// Called on `bobcat:worker` with one native module callback's answer.
pub(super) const WORKER_MODULE_CALLBACK_EXPORT: &str = "__BobcatNativeModuleCallback";

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
    js_runtime.register_module_source(
        crate::esm::SELECTOR_QUERY_SPECIFIER,
        crate::esm::SELECTOR_QUERY_SOURCE,
    )?;
    js_runtime.register_module_source(
        "bobcat:lynx-modules",
        crate::esm::runtime_source!("lynx-modules"),
    )?;
    js_runtime.register_module_source(GLOBAL_EVENT_MODULE_SPECIFIER, GLOBAL_EVENT_MODULE_SOURCE)?;
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
/// and the three members that are a worker's whole outward surface.
///
/// There is no document member here and no way to add one: this realm is on
/// another runtime, on another thread, and the document is neither `Send` nor
/// reachable from anything the closures below capture. The native-module
/// member is no exception: it names a module and hands over text, and what
/// serves it is the embedder's own thread.
pub(super) fn install_worker_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    timers: &Rc<TimerState>,
    closing: &Rc<Cell<bool>>,
    host: &HostOutbox,
    inbox: &mpsc::WeakUnboundedSender<WorkerMessage>,
    mut post: impl FnMut(HostValue) + 'static,
) -> Result<(), ScriptError> {
    install_timer_members(engine, js_runtime, timers)?;
    install_native_modules(engine, js_runtime, host, inbox)?;

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

/// Installs the one member `NativeModules.<module>.<method>(...)` reaches the
/// embedder through.
///
/// Everything crosses as text, because everything here is JavaScript's: the
/// arguments are the realm's own JSON, and the function arguments are named by
/// the indices they occupied rather than carried. What Rust builds out of that
/// is one [`ModuleCall`] with one [`ModuleCallback`] per index, and the notice
/// it rides is the same one a source request uses — so a view that has ended
/// drops it, and the dropped callbacks release their functions.
fn install_native_modules(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    host: &HostOutbox,
    inbox: &mpsc::WeakUnboundedSender<WorkerMessage>,
) -> Result<(), ScriptError> {
    const NAME: &str = "bobcat-internal:worker.invokeNativeModule";
    let host = host.clone();
    let inbox = inbox.clone();
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
                        .map(|index| {
                            ModuleCallback::new(call, index, inbox.clone(), host.token().clone())
                        })
                        .map_err(|_| format!("{NAME} expects argument indices for argument 4"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            host.notify(ViewNotice::NativeModuleCall {
                module,
                call: ModuleCall {
                    method,
                    arguments: call_arguments,
                    callbacks,
                },
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
