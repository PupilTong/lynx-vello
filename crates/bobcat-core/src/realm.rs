//! Opening a realm, on either engine thread.
//!
//! Every realm is opened by [`open_realm`]: a view's MTS realm on
//! `bobcat-main`, and the BTS and every plain `Worker` on `bobcat-workers`.
//! Each gets the same core under `bobcat-internal:host` — `requestScriptFrame`,
//! the timer pair, the `Future` members, `fetchResource`, and the two members
//! `bobcat:module` is written over — installed here and nowhere else.
//!
//! What differs between realm kinds is the host modules installed after the
//! core, and those are the caller's: [`open_realm`] takes them as a closure.
//! An MTS realm's are its document, style, startup and `Worker` members; a
//! worker realm's is `bobcat-internal:worker`. Nothing here names a kind or
//! branches on one. The core is told one thing about its realm, the key its
//! display-frame demand is reported under: `None` for a view's MTS realm and
//! the worker's own key for a worker realm.

use std::rc::Rc;
use std::sync::Arc;

use crate::background::WorkerKey;
use crate::future::FutureTable;
use crate::jobs::JsThreadHandle;
use crate::link::{HostOutbox, ViewNotice};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;
use crate::timers::{TimerState, install_timer_members};

/// What every realm holds, whatever else its owner keeps beside it: the realm
/// itself and the two tables its core members write to.
///
/// `engine` is declared first, so an owner that holds this as its first field
/// frees the realm, and with it every host function that holds a clone of
/// either table, before anything declared after it.
pub(crate) struct RealmCore {
    pub(crate) engine: ScriptEngine,
    /// What `setTimer` arms and the owner's epilogue fires.
    pub(crate) timers: Rc<TimerState>,
    /// Every host-backed operation this realm holds a `Future` for, every
    /// `fetchResource` included.
    pub(crate) futures: Rc<FutureTable>,
}

/// Opens one realm on `js`: creates it, enables module loading, installs the
/// core, then runs `host_modules`, and answers with the core and whatever
/// `host_modules` returned.
///
/// `host` is what the core's members reach the view through. Its token is the
/// end signal a `Future.wait` and a `require` park against: the view's for an
/// MTS realm, the worker's own for a worker realm, so a wait ends with the
/// realm that asked. `thread` is the engine thread whose jobs this realm's
/// entries are.
pub(crate) fn open_realm<T>(
    js: &mut ScriptRuntime,
    host: &HostOutbox,
    thread: JsThreadHandle,
    frame_target: Option<WorkerKey>,
    host_modules: impl FnOnce(&mut ScriptEngine, &mut ScriptRuntime) -> Result<T, ScriptError>,
) -> Result<(RealmCore, T), ScriptError> {
    let mut engine = js
        .create_realm()
        .map_err(|error| context_of("creating the realm", error))?;
    // Before any member: registering one does not read the normalizer, so the
    // order is not observable, and it leaves no stretch in which the realm
    // exists without module loading.
    engine.enable_module_loading();
    let (timers, futures) = install_realm_core(&mut engine, js, host, thread, frame_target)?;
    let installed = host_modules(&mut engine, js)?;
    Ok((
        RealmCore {
            engine,
            timers,
            futures,
        },
        installed,
    ))
}

/// Installs the members every realm has, and answers with the two tables they
/// write to.
fn install_realm_core(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    host: &HostOutbox,
    thread: JsThreadHandle,
    frame_target: Option<WorkerKey>,
) -> Result<(Rc<TimerState>, Rc<FutureTable>), ScriptError> {
    let frames = host.clone();
    crate::script_frames::install(engine, js, move |pending| {
        frames.notify(ViewNotice::ScriptFrameDemand {
            worker: frame_target,
            pending,
        });
    })
    .map_err(|error| context_of("installing animation frames", error))?;
    let timers = Rc::new(TimerState::new());
    install_timer_members(engine, js, &timers)
        .map_err(|error| context_of("installing the timer members", error))?;
    let futures = Rc::new(FutureTable::new());
    crate::future::install(engine, js, &futures, host.token().clone(), thread.clone())
        .map_err(|error| context_of("installing Future", error))?;
    // Over the table above rather than a wait of its own: what a fetch hands
    // JavaScript is a future of this realm's.
    crate::fetch::install(engine, js, host, &futures)
        .map_err(|error| context_of("installing fetchResource", error))?;
    crate::require::install(engine, js, host.clone(), thread)
        .map_err(|error| context_of("installing require", error))?;
    Ok((timers, futures))
}

/// Prefixes a failure with what the host was doing, in the format
/// `MainThreadError` gives the errors that reach an embedder through a view.
pub(crate) fn context_of(context: &str, mut error: ScriptError) -> ScriptError {
    error.message = Arc::from(format!("{context}: {}", error.message));
    error
}
