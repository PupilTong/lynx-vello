//! What a worker realm is made of, on the group's *other* `QuickJS` runtime.
//!
//! Only that. Which realms exist, and when, is [`super::thread`]; the handle
//! the rest of the engine holds them by is [`super::Background`].

use std::cell::Cell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use crate::background::WorkerKey;
use crate::link::HostOutbox;
use crate::main::quickjs::ScriptEngine;
use crate::script::ScriptError;

const WORKER_HOST_MODULE_SPECIFIER: &str = "bobcat-internal:worker";

pub(super) struct WorkerFlags {
    pub(super) scope_installed: Rc<Cell<bool>>,
    pub(super) closing: Rc<Cell<bool>>,
}

pub(super) fn install_worker_members(
    engine: &mut ScriptEngine,
    key: WorkerKey,
    host: &HostOutbox,
    name: String,
    mut post: impl FnMut(HostValue) + 'static,
) -> Result<WorkerFlags, ScriptError> {
    crate::native_module::install(engine, host, Some(key))?;

    let mut name = Some(name);
    let scope_installed = Rc::new(Cell::new(false));
    let installed = Rc::clone(&scope_installed);
    engine.register_host_module_function(
        WORKER_HOST_MODULE_SPECIFIER,
        "workerName",
        0,
        move |_arguments| {
            installed.set(true);
            Ok(name.take().map_or(HostValue::Undefined, HostValue::String))
        },
    )?;

    engine.register_host_module_function(
        WORKER_HOST_MODULE_SPECIFIER,
        "postWorkerMessage",
        1,
        move |arguments| {
            post(
                arguments
                    .first()
                    .map_or(HostValue::Undefined, |data| data.to_value()),
            );
            Ok(HostValue::Undefined)
        },
    )?;

    engine.register_host_module_shared_object(WORKER_HOST_MODULE_SPECIFIER, "sharedObject")?;

    let closing = Rc::new(Cell::new(false));
    let closer = Rc::clone(&closing);
    engine.register_host_module_function(
        WORKER_HOST_MODULE_SPECIFIER,
        "closeWorker",
        0,
        move |_arguments| {
            closer.set(true);
            Ok(HostValue::Undefined)
        },
    )?;
    Ok(WorkerFlags {
        scope_installed,
        closing,
    })
}
