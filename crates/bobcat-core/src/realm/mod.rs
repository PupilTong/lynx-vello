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
//! names a kind or branches on one. The core is told three things about its
//! realm: the key its display-frame demand is reported under — `None` for a
//! view's MTS realm and the worker's own key for a worker realm — the
//! [`ScriptSource`] its diagnostics name, and the [`Opener`] its fetches are
//! timed as.
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
use crate::canvas::{CanvasHost, CanvasTable, ReadWait};
use crate::esm::{Export, HOST_MODULE_SPECIFIER};
use crate::future::FutureTable;
use crate::jobs::JsThreadHandle;
use crate::link::{HostOutbox, ViewNotice};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::performance::Opener;
use crate::script::ScriptError;
use crate::timers::{TimerState, install_timer_members};
use crate::view::{EngineEvent, ScriptSource};

pub(crate) struct RealmCore {
    pub(crate) engine: ScriptEngine,
    pub(crate) children: Rc<ChildRealms>,
    pub(crate) timers: Rc<TimerState>,
    pub(crate) futures: Rc<FutureTable>,
    pub(crate) canvases: Rc<CanvasTable>,
    #[cfg(feature = "devtools")]
    pub(crate) console: Rc<crate::devtools::runtime::ConsoleTap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RealmId {
    Page,
    Child(ChildKey),
}

impl RealmId {
    pub(crate) fn listener_key(self) -> terminus::link::event::ListenerKey {
        match self {
            Self::Page => terminus::link::event::ListenerKey(0),
            Self::Child(key) => {
                debug_assert_ne!(key.number(), 0, "child keys start at 1");
                terminus::link::event::ListenerKey(key.number())
            }
        }
    }

    pub(crate) fn from_listener_key(key: terminus::link::event::ListenerKey) -> Self {
        match key.0 {
            0 => Self::Page,
            number => Self::Child(ChildKey::from_number(number)),
        }
    }
}

impl RealmCore {
    pub(crate) fn settle_drawn_canvases(&self) {
        self.canvases.settle_drawn();
        self.children.settle_drawn_canvases();
    }

    pub(crate) fn realm_ids(&self) -> impl Iterator<Item = RealmId> + use<> {
        std::iter::once(RealmId::Page).chain(self.children.keys().into_iter().map(RealmId::Child))
    }

    pub(crate) fn call_module_export(
        &self,
        js: &mut ScriptRuntime,
        id: RealmId,
        export: Export,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        match id {
            RealmId::Page => Some(self.engine.call_module_export(js, export, arguments)),
            RealmId::Child(key) => self.children.call_module_export(js, key, export, arguments),
        }
    }

    pub(crate) fn call_module_export_before_operation(
        &self,
        js: &mut ScriptRuntime,
        id: RealmId,
        export: Export,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        match id {
            RealmId::Page => Some(
                self.engine
                    .call_module_export_before_operation(js, export, arguments),
            ),
            RealmId::Child(key) => self
                .children
                .call_module_export_before_operation(js, key, export, arguments),
        }
    }

    pub(crate) fn call_module_export_nested(
        &self,
        id: RealmId,
        export: Export,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        match id {
            RealmId::Page => Some(self.engine.call_module_export_nested(export, arguments)),
            RealmId::Child(key) => self
                .children
                .call_module_export_nested(key, export, arguments),
        }
    }

    pub(crate) fn checkpoint_after_calls(
        &self,
        js: &mut ScriptRuntime,
        id: RealmId,
    ) -> Option<Result<(), ScriptError>> {
        match id {
            RealmId::Page => Some(self.engine.checkpoint_after_calls(js)),
            RealmId::Child(key) => self.children.checkpoint_after_calls(js, key),
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "what a realm is opened with")]
pub(crate) fn open_realm<T>(
    js: &mut ScriptRuntime,
    host: &HostOutbox,
    thread: JsThreadHandle,
    frame_target: Option<WorkerKey>,
    names: RealmNames,
    child_hosts: Option<Rc<dyn ChildHostModules>>,
    canvas_host: CanvasHost,
    host_modules: impl FnOnce(&mut ScriptEngine) -> Result<T, ScriptError>,
) -> Result<(RealmCore, T), ScriptError> {
    let mut engine = js
        .create_realm()
        .map_err(|error| context_of("creating the realm", error))?;
    engine.enable_module_loading();
    let children = Rc::new(ChildRealms::new());
    engine.set_children(Rc::downgrade(&children));
    #[cfg(feature = "devtools")]
    let console = Rc::clone(&names.console);
    let CoreTables {
        timers,
        futures,
        canvases,
    } = install_realm_core(
        &mut engine,
        host,
        thread,
        FrameDemand::Report(frame_target),
        names,
        &Rc::downgrade(&children),
        child_hosts,
        canvas_host,
    )?;
    let installed = host_modules(&mut engine)?;
    Ok((
        RealmCore {
            engine,
            children,
            timers,
            futures,
            canvases,
            #[cfg(feature = "devtools")]
            console,
        },
        installed,
    ))
}

#[derive(Clone, Copy)]
pub(crate) enum FrameDemand {
    Report(Option<WorkerKey>),
    Ignored,
}

pub(crate) struct RealmNames {
    pub(crate) source: ScriptSource,
    pub(crate) opener: Opener,
    #[cfg(feature = "devtools")]
    pub(crate) console: Rc<crate::devtools::runtime::ConsoleTap>,
}

pub(crate) struct CoreTables {
    pub(crate) timers: Rc<TimerState>,
    pub(crate) futures: Rc<FutureTable>,
    pub(crate) canvases: Rc<CanvasTable>,
}

#[expect(clippy::too_many_arguments, reason = "what a realm's core is made of")]
fn install_realm_core(
    engine: &mut ScriptEngine,
    host: &HostOutbox,
    thread: JsThreadHandle,
    frames: FrameDemand,
    names: RealmNames,
    children: &Weak<ChildRealms>,
    child_hosts: Option<Rc<dyn ChildHostModules>>,
    canvas_host: CanvasHost,
) -> Result<CoreTables, ScriptError> {
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
    crate::fetch::install(engine, host, &futures, names.opener)
        .map_err(|error| context_of("installing fetchResource", error))?;
    crate::require::install(engine, host.clone(), thread.clone())
        .map_err(|error| context_of("installing require", error))?;
    install_diagnostics(engine, host, &names)
        .map_err(|error| context_of("installing the diagnostics members", error))?;
    let RealmNames {
        source,
        opener,
        #[cfg(feature = "devtools")]
        console,
    } = names;
    let canvases = Rc::new(CanvasTable::new(
        &canvas_host,
        ReadWait::new(host.token(), &thread),
        std::sync::Arc::clone(host.canvas_hand_over()),
    ));
    crate::canvas::install(engine, &canvases)
        .map_err(|error| context_of("installing the canvas members", error))?;
    child::install(
        engine,
        ShadowRealmHook {
            children: Weak::clone(children),
            host: host.clone(),
            thread,
            source,
            opener,
            child_hosts,
            canvas_host,
            #[cfg(feature = "devtools")]
            console,
        },
    )
    .map_err(|error| context_of("installing ShadowRealm", error))?;
    Ok(CoreTables {
        timers,
        futures,
        canvases,
    })
}

fn install_diagnostics(
    engine: &mut ScriptEngine,
    host: &HostOutbox,
    names: &RealmNames,
) -> Result<(), ScriptError> {
    for (name, is_error) in [("reportScriptError", true), ("logScriptMessage", false)] {
        let reporting = host.clone();
        let source = names.source.clone();
        #[cfg(feature = "devtools")]
        let console = Rc::clone(&names.console);
        engine.register_host_module_function(HOST_MODULE_SPECIFIER, name, 2, move |arguments| {
            let level = string_argument(name, arguments, 0)?.to_owned();
            let message = string_argument(name, arguments, 1)?.to_owned();
            #[cfg(feature = "devtools")]
            if console.is_on() {
                let call = if is_error {
                    crate::devtools::runtime::ConsoleCall::Report
                } else {
                    crate::devtools::runtime::ConsoleCall::Log
                };
                console.message(call, &level, &message);
            }
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
        })?;
    }
    Ok(())
}

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
