//! What a worker realm is made of, on the group's *other* `QuickJS` runtime.
//!
//! Only that. Which realms exist, and when, is [`super::thread`]; the handle
//! the rest of the engine holds them by is [`super::Background`].

use std::cell::Cell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use crate::background::WorkerKey;
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

/// The worker realm's own host module. A worker realm declares three: this
/// one, with the members only a worker has; `bobcat-internal:host`, which
/// carries the core [`crate::realm::open_realm`] installs in every realm and
/// none of the MTS realm's document members; and
/// [`NATIVE_MODULES_HOST_SPECIFIER`](crate::esm::NATIVE_MODULES_HOST_SPECIFIER),
/// which every realm kind declares, with the member a native module call
/// reaches the embedder through.
const WORKER_HOST_MODULE_SPECIFIER: &str = "bobcat-internal:worker";
/// Called on `bobcat:worker`, in a worker realm, with one message value.
pub(super) const WORKER_DELIVER_EXPORT: &str = "__BobcatDeliverWorkerMessage";

/// What a worker realm's own members report back to the thread: two flags,
/// each written from inside the realm the thread acts on and read by the
/// thread once the call that set it has returned.
pub(super) struct WorkerFlags {
    /// Set as `bobcat:worker` reads `workerName`, which is that module's last
    /// statement: whether this realm has the global scope a posted message
    /// is delivered to. The engine installs that scope in no realm, so it is
    /// set only in a realm whose script imported the module, `bobcat:bts`
    /// among them, and only once the whole module has run.
    pub(super) scope_installed: Rc<Cell<bool>>,
    /// Set by `closeWorker`. A flag, not a teardown: the call runs inside the
    /// realm it would tear down.
    pub(super) closing: Rc<Cell<bool>>,
}

/// Installs a worker realm's own host modules, `bobcat-internal:worker` and
/// `bobcat-internal:native-modules`: the members that are a worker's whole
/// outward surface beyond the core [`crate::realm::open_realm`] installed
/// under `bobcat-internal:host`. Every worker gets the same members, the BTS
/// included: what sets the BTS apart is data the MTS realm posts to it, not
/// anything installed here. Answers with the flags the members set.
///
/// `bobcat-internal:native-modules` is [`crate::native_module::install`]'s,
/// the one every realm kind is given; a call a worker makes names the
/// worker's `key`, which is how the view answers it through the worker's
/// inbox.
///
/// `workerName` hands its string over once and keeps nothing, as an MTS
/// realm's page data members do: `bobcat:worker` reads it as the last
/// statement it evaluates, and nothing else of the engine's reads it. So the
/// read is also how the thread learns that the whole module has run in this
/// realm, which is what [`WorkerFlags::scope_installed`] records.
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
    name: String,
    mut post: impl FnMut(HostValue) + 'static,
) -> Result<WorkerFlags, ScriptError> {
    crate::native_module::install(engine, js_runtime, host, Some(key))?;

    let mut name = Some(name);
    let scope_installed = Rc::new(Cell::new(false));
    let installed = Rc::clone(&scope_installed);
    engine.register_host_module_function(
        js_runtime,
        WORKER_HOST_MODULE_SPECIFIER,
        "workerName",
        0,
        Box::new(move |_arguments| {
            installed.set(true);
            Ok(name.take().map_or(HostValue::Undefined, HostValue::String))
        }),
    )?;

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
    Ok(WorkerFlags {
        scope_installed,
        closing,
    })
}
