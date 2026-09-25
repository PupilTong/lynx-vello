//! What a worker realm is made of, on the group's *other* `QuickJS` runtime.
//!
//! Only that. Which realms exist, and when, is [`super::thread`]; the handle
//! the rest of the engine holds them by is [`super::Background`].

use std::cell::Cell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use crate::background::{BackgroundStart, WorkerKey, WorkerRole};
use crate::esm::{BTS_MODULE_SPECIFIER, TIMER_MODULE_SPECIFIER, WORKER_MODULE_SPECIFIER};
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

/// The worker realm's own host module. A worker realm declares three: this
/// one, with the members only a worker has; `bobcat-internal:host`, which
/// carries the core [`crate::realm::open_realm`] installs in every realm and
/// none of the MTS realm's document members; and
/// [`NATIVE_MODULES_HOST_SPECIFIER`](crate::esm::NATIVE_MODULES_HOST_SPECIFIER),
/// which every realm kind declares, with the embedder's module table.
const WORKER_HOST_MODULE_SPECIFIER: &str = "bobcat-internal:worker";
/// Called on `bobcat:worker`, in a worker realm, with one message value.
pub(super) const WORKER_DELIVER_EXPORT: &str = "__BobcatDeliverWorkerMessage";

/// The root module of a worker realm: what the realm evaluates as it opens,
/// under [`WORKER_BOOT_SPECIFIER`](crate::esm::WORKER_BOOT_SPECIFIER), and
/// whose evaluation is the worker's boot.
///
/// Every worker's root starts with the same two imports, which put the global
/// scope — its `name` included — and the timer globals in place before
/// anything of the worker's own runs. Then its role: a dedicated worker's
/// root imports its script by the URL it was requested by, and a BTS's root
/// imports the registered module `bobcat:bts`, the whole BTS bootstrap.
///
/// The script is not written into the root. The worker's own task completes
/// the module the root's `import` asks for, from the answer to the request
/// `createWorker` made and under that request's name, the way a view's own
/// task completes its MTS entry. A module completed in a realm is that realm's
/// own source and is never named on the runtime, so two views that answer one
/// URL with different bytes each run their own, and a worker leaves no
/// registration behind. The script keeps its own line numbers, and its
/// `import.meta.url` is the response URL.
pub(super) fn worker_boot_source(role: &WorkerRole) -> String {
    let role = match role {
        WorkerRole::Background(_) => format!(r#"import "{BTS_MODULE_SPECIFIER}";"#),
        WorkerRole::Dedicated { url, .. } => {
            let url = serde_json::to_string(url)
                .expect("serializing a Rust string as a JavaScript string cannot fail");
            format!("await import({url});")
        }
    };
    format!(
        r#"import "{WORKER_MODULE_SPECIFIER}";
import "{TIMER_MODULE_SPECIFIER}";
{role}
"#
    )
}

/// Installs a worker realm's own host modules, `bobcat-internal:worker` and
/// `bobcat-internal:native-modules`: the members that are a worker's whole
/// outward surface beyond the core [`crate::realm::open_realm`] installed
/// under `bobcat-internal:host`. The BTS and a plain `Worker` get the same
/// members. A BTS's members answer with its `background` data; a plain
/// `Worker` has none, so its `backgroundEntry` and three screen members
/// answer `undefined` and its `nativeModuleTable` an empty table. Answers
/// with the flag `closeWorker` sets.
///
/// `bobcat-internal:native-modules` is [`crate::native_module::install`]'s,
/// the one every realm kind is given; a call a worker makes names the
/// worker's `key`, which is how the view answers it through the worker's
/// inbox.
///
/// `workerName` and `backgroundEntry` each hand their string over once and
/// keep nothing, as an MTS realm's page data members do: `bobcat:worker`
/// reads the name and `bobcat:bts` the entry, as each is evaluated. The
/// screen members answer numbers, primitives Rust owns, rather than an
/// object the realm would have to be handed and parse.
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
    background: Option<BackgroundStart>,
    mut post: impl FnMut(HostValue) + 'static,
) -> Result<Rc<Cell<bool>>, ScriptError> {
    let (entry, screen, table) = match background {
        Some(BackgroundStart {
            entry,
            screen,
            native_modules,
        }) => (entry, Some(screen), native_modules),
        None => (None, None, String::new()),
    };
    crate::native_module::install(engine, js_runtime, host, Some(key), table)?;

    for (member, mut value) in [("workerName", Some(name)), ("backgroundEntry", entry)] {
        engine.register_host_module_function(
            js_runtime,
            WORKER_HOST_MODULE_SPECIFIER,
            member,
            0,
            Box::new(move |_arguments| {
                Ok(value.take().map_or(HostValue::Undefined, HostValue::String))
            }),
        )?;
    }
    for (member, value) in [
        ("pixelRatio", screen.map(|screen| screen.pixel_ratio)),
        ("pixelWidth", screen.map(|screen| screen.pixel_width)),
        ("pixelHeight", screen.map(|screen| screen.pixel_height)),
    ] {
        let value = value.map_or(HostValue::Undefined, screen_number);
        engine.register_host_module_function(
            js_runtime,
            WORKER_HOST_MODULE_SPECIFIER,
            member,
            0,
            Box::new(move |_arguments| Ok(value.clone())),
        )?;
    }

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

/// One screen number as the MTS boot module's literal for it evaluates.
///
/// Boot writes an `f32` into its source in the shortest decimal that reads
/// back as that `f32`, and JavaScript reads the decimal as the nearest
/// `f64`, which is not always the `f32`'s own value: `1.1_f32` is written
/// `1.1`, while `f64::from(1.1_f32)` is `1.100000023841858`. Reading the
/// same decimal here keeps the BTS's `SystemInfo` equal to the MTS's.
fn screen_number(value: f32) -> HostValue {
    HostValue::Number(
        value
            .to_string()
            .parse()
            .expect("an f32's decimal form parses as an f64"),
    )
}
