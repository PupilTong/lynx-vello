//! Opening a realm, on either engine thread, and driving it once it is open.
//!
//! Every realm is opened by [`open_realm`]: a view's MTS realm on
//! `bobcat-main`, and the BTS and every plain `Worker` on `bobcat-workers`.
//! Each gets the same core under `bobcat-internal:host` — `requestScriptFrame`,
//! the timer pair, the `Future` members, `fetchResource` and `resolveUrl`, the
//! two members `bobcat:module` is written over, and the two
//! `bobcat:diagnostics` is written over — installed here and nowhere else, and
//! the TC39 `ShadowRealm` global,
//! every `new ShadowRealm()` of which is a child realm furnished with that
//! same core over tables of its own ([`child`]) and kept in the
//! [`RealmCore`]'s list for the owner's epilogue to service.
//!
//! What differs between realm kinds is the host modules installed after the
//! core, and those are the caller's: [`open_realm`] takes them as a closure,
//! and the ones its `ShadowRealm` children get beyond the core as a
//! [`ChildHostModules`] (an MTS realm's document and `Worker` members,
//! `None` for a worker).
//! An MTS realm's are its document, style, startup and `Worker` members; a
//! worker realm's are the members of `bobcat-internal:worker`. Both kinds
//! also install `bobcat-internal:native-modules` through
//! [`crate::native_module::install`], the same member in each. Nothing here
//! names a kind or branches on one. The core is told two things about its
//! realm: the key its display-frame demand is reported under — `None` for a
//! view's MTS realm and the worker's own key for a worker realm — and the
//! [`ScriptSource`] its diagnostics name.
//!
//! Once a realm is open, its owner — a view's page or a worker — drives it
//! through [`owner`]: the tasks, the one job per entry, the epilogue with the
//! module loads and future settles it spawns, the end and the release are
//! written there once for both threads. What a failure in either kind of
//! realm is reported as is [`policy`]'s: one table per realm kind.

pub(crate) mod child;
pub(crate) mod owner;
pub(crate) mod policy;

use std::rc::{Rc, Weak};

use quickjs_rust_bridge::{HostArgument, HostValue};

use self::child::{ChildHostModules, ChildKey, ChildRealms, ShadowRealmHook};
use self::policy::context_of;
use crate::background::WorkerKey;
use crate::esm::HOST_MODULE_SPECIFIER;
use crate::future::FutureTable;
use crate::jobs::JsThreadHandle;
use crate::link::{HostOutbox, ViewNotice};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;
use crate::timers::{TimerState, install_timer_members};
use crate::view::{EngineEvent, ScriptSource};

/// What every realm holds, whatever else its owner keeps beside it: the realm
/// itself and the two tables its core members write to.
///
/// `engine` is declared first, so an owner that holds this as its first field
/// frees the realm, and with it every host function that holds a clone of
/// either table, before anything declared after it. `children` comes next:
/// the realm's `ShadowRealm` objects and wrapped functions are freed with it,
/// and then the children they named.
pub(crate) struct RealmCore {
    pub(crate) engine: ScriptEngine,
    /// The realm's `ShadowRealm` children, grandchildren included, which the
    /// owner's epilogue services. Only this field holds the list strongly:
    /// the hook that fills it holds it weakly, so it goes with the core.
    pub(crate) children: Rc<ChildRealms>,
    /// What `setTimer` arms and the owner's epilogue fires.
    pub(crate) timers: Rc<TimerState>,
    /// Every host-backed operation this realm holds a `Future` for, every
    /// `fetchResource` included.
    pub(crate) futures: Rc<FutureTable>,
}

/// One JavaScript context of an owner's realm tree on this runtime: the
/// owner's own realm, or one of its `ShadowRealm` children, grandchildren
/// included, by the key [`ChildRealms`] keeps it under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RealmId {
    /// The owner's own realm: a view's page realm, or a worker's.
    Page,
    /// A `ShadowRealm` child.
    Child(ChildKey),
}

impl RealmId {
    /// This id as one opaque number, for a table that files what a realm
    /// reports under a [`terminus::link::event::ListenerKey`]: 0 for [`Self::Page`],
    /// the child key's own number for a child. Child keys are handed out
    /// from 1 and never reused, so no child is ever 0 and a released child's
    /// number is never a newer child's.
    pub(crate) fn listener_key(self) -> terminus::link::event::ListenerKey {
        match self {
            Self::Page => terminus::link::event::ListenerKey(0),
            Self::Child(key) => {
                debug_assert_ne!(key.number(), 0, "child keys start at 1");
                terminus::link::event::ListenerKey(key.number())
            }
        }
    }

    /// The realm [`Self::listener_key`] answered `key` for.
    pub(crate) fn from_listener_key(key: terminus::link::event::ListenerKey) -> Self {
        match key.0 {
            0 => Self::Page,
            number => Self::Child(ChildKey::from_number(number)),
        }
    }
}

impl RealmCore {
    /// Every realm of this core, the owner's own first, then its children in
    /// construction order.
    pub(crate) fn realm_ids(&self) -> impl Iterator<Item = RealmId> + use<> {
        std::iter::once(RealmId::Page).chain(self.children.keys().into_iter().map(RealmId::Child))
    }

    /// Calls an export of a module the realm `id` has loaded, through
    /// [`ScriptEngine::call_module_export`] for the owner's own realm and
    /// [`ChildRealms::call_module_export`] for a child, so the checkpoint
    /// that ends the call is that realm's. `None` is a child that is gone,
    /// which drops the call; the owner's own realm is always there.
    pub(crate) fn call_module_export(
        &mut self,
        js: &mut ScriptRuntime,
        id: RealmId,
        module_specifier: &str,
        export_name: &str,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        match id {
            RealmId::Page => {
                Some(
                    self.engine
                        .call_module_export(js, module_specifier, export_name, arguments),
                )
            }
            RealmId::Child(key) => {
                self.children
                    .call_module_export(js, key, module_specifier, export_name, arguments)
            }
        }
    }

    /// [`Self::call_module_export`] without the checkpoint that ends the
    /// call: [`ScriptEngine::call_module_export_before_operation`] for the
    /// owner's own realm and [`ChildRealms::call_module_export_before_operation`]
    /// for a child. The caller ends the run with
    /// [`Self::checkpoint_after_calls`] for each realm it called.
    pub(crate) fn call_module_export_before_operation(
        &mut self,
        js: &mut ScriptRuntime,
        id: RealmId,
        module_specifier: &str,
        export_name: &str,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        match id {
            RealmId::Page => Some(self.engine.call_module_export_before_operation(
                js,
                module_specifier,
                export_name,
                arguments,
            )),
            RealmId::Child(key) => self.children.call_module_export_before_operation(
                js,
                key,
                module_specifier,
                export_name,
                arguments,
            ),
        }
    }

    /// The checkpoint of the realm `id` that ends a run of
    /// [`Self::call_module_export_before_operation`] calls. `None` is a
    /// child that is gone.
    pub(crate) fn checkpoint_after_calls(
        &mut self,
        js: &mut ScriptRuntime,
        id: RealmId,
    ) -> Option<Result<(), ScriptError>> {
        match id {
            RealmId::Page => Some(self.engine.checkpoint_after_calls(js)),
            RealmId::Child(key) => self.children.checkpoint_after_calls(js, key),
        }
    }
}

/// Opens one realm on `js`: creates it, enables module loading, installs the
/// core, then runs `host_modules`, and answers with the core and whatever
/// `host_modules` returned.
///
/// `host` is what the core's members reach the view through. Its token is the
/// end signal a `Future.wait` and a `require` park against: the view's for an
/// MTS realm, the worker's own for a worker realm, so a wait ends with the
/// realm that asked. `thread` is the engine thread whose jobs this realm's
/// entries are. `source` is what this realm's console output and reports are
/// named by when they reach the embedder. `child_hosts` is what every
/// `ShadowRealm` child of this realm, and of its children, gets beyond the
/// core.
pub(crate) fn open_realm<T>(
    js: &mut ScriptRuntime,
    host: &HostOutbox,
    thread: JsThreadHandle,
    frame_target: Option<WorkerKey>,
    source: ScriptSource,
    child_hosts: Option<Rc<dyn ChildHostModules>>,
    host_modules: impl FnOnce(&mut ScriptEngine) -> Result<T, ScriptError>,
) -> Result<(RealmCore, T), ScriptError> {
    let mut engine = js
        .create_realm()
        .map_err(|error| context_of("creating the realm", error))?;
    // Before any member: registering one does not read the normalizer, so the
    // order is not observable, and it leaves no stretch in which the realm
    // exists without module loading.
    engine.enable_module_loading();
    let children = Rc::new(ChildRealms::new());
    engine.set_children(Rc::downgrade(&children));
    let (timers, futures) = install_realm_core(
        &mut engine,
        host,
        thread,
        FrameDemand::Report(frame_target),
        source,
        &Rc::downgrade(&children),
        child_hosts,
    )?;
    let installed = host_modules(&mut engine)?;
    Ok((
        RealmCore {
            engine,
            children,
            timers,
            futures,
        },
        installed,
    ))
}

/// What a realm's `requestScriptFrame` does with what it is told.
#[derive(Clone, Copy)]
pub(crate) enum FrameDemand {
    /// Reports it to the view under this key: `None` for a view's MTS realm,
    /// the worker's own key for a worker realm.
    Report(Option<WorkerKey>),
    /// Nothing, in a `ShadowRealm` child. The demand is one flag per realm
    /// key and a child has no key of its own, so a child that reported under
    /// its parent's would cancel the parent's demand with its own `false`.
    Ignored,
}

/// Installs the members every realm has, the `ShadowRealm` constructor last,
/// and answers with the two tables they write to.
///
/// One function for an owner's realm and for each `ShadowRealm` child, which
/// differ only in `frames`. A child is furnished from inside its parent's
/// JavaScript, which is why nothing here takes the runtime. `children` is the
/// list every `new ShadowRealm()` of this realm, and of its children, joins,
/// and `child_hosts` what each of them gets beyond the core.
fn install_realm_core(
    engine: &mut ScriptEngine,
    host: &HostOutbox,
    thread: JsThreadHandle,
    frames: FrameDemand,
    source: ScriptSource,
    children: &Weak<ChildRealms>,
    child_hosts: Option<Rc<dyn ChildHostModules>>,
) -> Result<(Rc<TimerState>, Rc<FutureTable>), ScriptError> {
    let notices = host.clone();
    crate::script_frames::install(engine, move |pending| {
        if let FrameDemand::Report(worker) = frames {
            notices.notify(ViewNotice::ScriptFrameDemand { worker, pending });
        }
    })
    .map_err(|error| context_of("installing animation frames", error))?;
    let timers = Rc::new(TimerState::new());
    install_timer_members(engine, &timers)
        .map_err(|error| context_of("installing the timer members", error))?;
    let futures = Rc::new(FutureTable::new());
    crate::future::install(engine, &futures, host.token().clone(), thread.clone())
        .map_err(|error| context_of("installing Future", error))?;
    // Over the table above rather than a wait of its own: what a fetch hands
    // JavaScript is a future of this realm's. `resolveUrl` beside it.
    crate::fetch::install(engine, host, &futures)
        .map_err(|error| context_of("installing fetchResource", error))?;
    crate::require::install(engine, host.clone(), thread.clone())
        .map_err(|error| context_of("installing require", error))?;
    install_diagnostics(engine, host, source.clone())
        .map_err(|error| context_of("installing the diagnostics members", error))?;
    child::install(
        engine,
        ShadowRealmHook {
            children: Weak::clone(children),
            host: host.clone(),
            thread,
            source,
            child_hosts,
        },
    )
    .map_err(|error| context_of("installing ShadowRealm", error))?;
    Ok((timers, futures))
}

/// Installs the two members this realm's console and `reportError` are written
/// over, in `bobcat:diagnostics`: `reportScriptError(level, message)` and
/// `logScriptMessage(level, message)`.
///
/// Each sends one engine event, named by `source`, through `host` to the
/// view's host, from whichever thread the realm is on. No other realm relays
/// it: a worker's report reaches the embedder while the main-thread realm is
/// busy, and it is ordered only against the other diagnostics of the same
/// realm.
fn install_diagnostics(
    engine: &mut ScriptEngine,
    host: &HostOutbox,
    source: ScriptSource,
) -> Result<(), ScriptError> {
    for (name, is_error, source) in [
        ("reportScriptError", true, source.clone()),
        ("logScriptMessage", false, source),
    ] {
        let reporting = host.clone();
        engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            name,
            2,
            Box::new(move |arguments| {
                let level = string_argument(name, arguments, 0)?.to_owned();
                let message = string_argument(name, arguments, 1)?.to_owned();
                // The event owns its source and leaves this thread, so each
                // takes a clone, which for a worker shares the URL rather
                // than copying it.
                let source = source.clone();
                reporting.engine_event(if is_error {
                    EngineEvent::ScriptReported {
                        source,
                        level,
                        message,
                    }
                } else {
                    EngineEvent::ConsoleMessage {
                        source,
                        level,
                        message,
                    }
                });
                Ok(HostValue::Undefined)
            }),
        )?;
    }
    Ok(())
}

/// One argument of a host call as a string. A missing, `undefined` or `null`
/// argument reads as the empty string; any other value that is not a string
/// is refused with a message naming `function` and the argument's position.
pub(crate) fn string_argument<'a>(
    function: &str,
    arguments: &[HostArgument<'a>],
    index: usize,
) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(&HostArgument::String(value)) => Ok(value),
        None | Some(HostArgument::Undefined | HostArgument::Null) => Ok(""),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}
