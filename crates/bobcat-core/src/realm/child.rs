//! A realm's `ShadowRealm` children: the hook every `new ShadowRealm()`
//! runs, the list the children live in, and what the owner's epilogue does
//! with them.
//!
//! A child is a fresh realm on its parent's runtime, which the bridge hands
//! to the host inside the construction (`HostInitializeShadowRealm`). The
//! host owns it from then on, so it is the realm core's: [`ChildRealms`] is
//! a field of [`super::RealmCore`], declared after the parent's engine, so
//! the parent realm — and with it every `ShadowRealm` object and wrapped
//! function it holds — is freed before any child is.
//!
//! A child gets the core every realm has, installed by the same
//! [`super::install_realm_core`] over tables of its own — its own timers,
//! its own futures — and the same host outbox, thread and [`ScriptSource`]
//! as its parent, so its requests are the view's or the worker's and its
//! diagnostics are named by its parent's source. The one member that does
//! nothing in a child is `requestScriptFrame`: the frame demand is reported
//! per realm key, and a child has none of its own, so its `false` would
//! cancel its parent's demand (`docs/tracking/deviations.md`). What else a
//! child gets is its owner's [`ChildHostModules`], if the owner names any:
//! an MTS realm's children get the page's document members, the same code
//! over the same document, with a `closeRealm` that sets the child's
//! `retired` flag, and the `Worker` members over a worker owner of the
//! child's own (`main/runtime/lib.rs`'s `DocumentHost`); a worker realm's get
//! nothing more, so a worker's child has no worker member. Grandchildren
//! are in the same flat list, and get the same modules, because every child
//! is the owner's to service.
//!
//! What those modules keep for one child — an MTS child's workers — is its
//! [`ChildHostState`], held by the child and dropped with it, after its
//! realm: freeing a child ends what it started.
//!
//! The owner's epilogue services every child (`realm/owner.rs`): their due
//! timers after the owner's own, their module requests and future settles
//! after the owner's, then what they — and, beside them, the owner — left
//! unhandled, reported under [`super::policy::Scene::ShadowRealm`]. Its last
//! step before the deadline releases every child nothing needs any more:
//! nothing of another realm refers to it, it has no timer armed, no load or
//! settle in flight and no worker of its own running. That is the only place
//! a child is freed while its owner lives (the MTS realm also runs it right
//! after a `frame`'s disconnection), and it is outside every JavaScript call,
//! because freeing a realm collects the whole runtime.
//!
//! Each child is kept behind an `Rc`, and every operation on one clones it
//! out of the list under a borrow that ends before the child runs anything,
//! then calls it through a shared borrow: a `new ShadowRealm()` the child's
//! code makes pushes a grandchild without meeting a borrow of the list, and a
//! call into a child from inside one of its own calls — a synthetic event
//! the child dispatched, which invites the child itself — finds the child in
//! the list, where it always is until its release.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use quickjs_rust_bridge::HostArgument;

use super::{CoreTables, FrameDemand, RealmNames};
use crate::canvas::{CanvasHost, CanvasTable, ReadWait};
use crate::clock::ClockInstant;
use crate::esm::Export;
use crate::future::{FutureTable, HostFuture, Outcome};
use crate::jobs::JsThreadHandle;
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::performance::{Opener, OpenerId};
use crate::script::ScriptError;
use crate::timers::TimerState;
use crate::view::ScriptSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ChildKey(u64);

impl ChildKey {
    pub(super) fn number(self) -> u64 {
        self.0
    }

    pub(super) fn from_number(number: u64) -> Self {
        Self(number)
    }
}

pub(crate) struct ChildRealm {
    engine: ScriptEngine,
    key: ChildKey,
    timers: Rc<TimerState>,
    futures: Rc<FutureTable>,
    canvases: Rc<CanvasTable>,
    retired: Rc<Cell<bool>>,
    in_flight: Cell<u32>,
    state: Option<Box<dyn ChildHostState>>,
}

pub(crate) struct ChildRealms {
    live: RefCell<Vec<Rc<ChildRealm>>>,
    next: Cell<u64>,
}

impl ChildRealms {
    pub(crate) fn new() -> Self {
        Self {
            live: RefCell::new(Vec::new()),
            next: Cell::new(1),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.live.borrow().len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.live.borrow().is_empty()
    }

    fn allocate(&self) -> ChildKey {
        let id = self.next.get();
        self.next.set(id.saturating_add(1));
        ChildKey(id)
    }

    pub(crate) fn keys(&self) -> Vec<ChildKey> {
        self.live.borrow().iter().map(|child| child.key).collect()
    }

    fn with_child<T>(&self, key: ChildKey, operation: impl FnOnce(&ChildRealm) -> T) -> Option<T> {
        let child = {
            let live = self.live.borrow();
            let index = live.binary_search_by_key(&key, |child| child.key).ok()?;
            Rc::clone(&live[index])
        };
        Some(operation(&child))
    }

    fn keep(&self, child: ChildRealm) {
        let mut live = self.live.borrow_mut();
        let index = live
            .binary_search_by_key(&child.key, |live| live.key)
            .unwrap_or_else(|index| index);
        live.insert(index, Rc::new(child));
    }

    pub(crate) fn run_due_timers(&self, js: &mut ScriptRuntime) -> Option<Vec<ScriptError>> {
        let mut failures: Option<Vec<ScriptError>> = None;
        for key in self.keys() {
            if !self.has_armed_timer(key) {
                continue;
            }
            let ran = self.with_child(key, |child| {
                crate::timers::run_due_timers(&child.engine, js, &child.timers)
            });
            if let Some(Some(thrown)) = ran {
                failures.get_or_insert_with(Vec::new).extend(thrown);
            }
        }
        failures
    }

    fn has_armed_timer(&self, key: ChildKey) -> bool {
        let live = self.live.borrow();
        live.binary_search_by_key(&key, |child| child.key)
            .is_ok_and(|index| live[index].timers.is_armed())
    }

    pub(crate) fn take_module_requests(&self) -> Vec<(ChildKey, String)> {
        let mut requests = Vec::new();
        for child in self.live.borrow().iter() {
            while let Some(url) = child.engine.take_module_request() {
                child.in_flight.set(child.in_flight.get().saturating_add(1));
                requests.push((child.key, url));
            }
        }
        requests
    }

    pub(crate) fn take_settle_requests(&self) -> Vec<(ChildKey, u32, HostFuture)> {
        let mut requests = Vec::new();
        for child in self.live.borrow().iter() {
            for (id, future) in child.futures.take_settle_requests() {
                child.in_flight.set(child.in_flight.get().saturating_add(1));
                requests.push((child.key, id, future));
            }
        }
        requests
    }

    pub(crate) fn complete_module(
        &self,
        js: &mut ScriptRuntime,
        key: ChildKey,
        url: &str,
        loaded: Result<(&str, &str), &str>,
    ) -> Option<Result<(), ScriptError>> {
        self.with_child(key, |child| {
            child.landed();
            child.engine.complete_module(js, url, loaded)
        })
    }

    pub(crate) fn deliver_future(
        &self,
        js: &mut ScriptRuntime,
        key: ChildKey,
        id: u32,
        outcome: Outcome,
    ) -> Option<Result<(), ScriptError>> {
        self.with_child(key, |child| {
            child.landed();
            crate::future::deliver(&child.engine, js, id, outcome)
        })
    }

    pub(crate) fn call_module_export(
        &self,
        js: &mut ScriptRuntime,
        key: ChildKey,
        export: Export,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        self.with_child(key, |child| {
            child.engine.call_module_export(js, export, arguments)
        })
    }

    pub(crate) fn call_module_export_before_operation(
        &self,
        js: &mut ScriptRuntime,
        key: ChildKey,
        export: Export,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        self.with_child(key, |child| {
            child
                .engine
                .call_module_export_before_operation(js, export, arguments)
        })
    }

    pub(crate) fn call_module_export_nested(
        &self,
        key: ChildKey,
        export: Export,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        self.with_child(key, |child| {
            child.engine.call_module_export_nested(export, arguments)
        })
    }

    pub(crate) fn checkpoint_after_calls(
        &self,
        js: &mut ScriptRuntime,
        key: ChildKey,
    ) -> Option<Result<(), ScriptError>> {
        self.with_child(key, |child| child.engine.checkpoint_after_calls(js))
    }

    pub(crate) fn take_unhandled_rejections(&self, js: &mut ScriptRuntime) -> Vec<ScriptError> {
        let mut failures = Vec::new();
        for key in self.keys() {
            let drained = self.with_child(key, |child| child.engine.take_unhandled_rejection(js));
            match drained {
                Some(Ok(executed)) if executed > 0 => js.mark_checkpoint(),
                Some(Err(error)) => failures.push(error),
                _ => {}
            }
        }
        failures
    }

    pub(crate) fn discard_rejections(&self, js: &mut ScriptRuntime) {
        for child in self.live.borrow().iter() {
            child.engine.discard_own_rejections(js);
        }
    }

    pub(crate) fn settle_drawn_canvases(&self) {
        for child in self.live.borrow().iter() {
            child.canvases.settle_drawn();
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<ClockInstant> {
        self.live
            .borrow()
            .iter()
            .filter_map(|child| child.timers.next_deadline())
            .min()
    }

    pub(crate) fn release_unneeded(&self) -> usize {
        let mut released = 0usize;
        loop {
            let freed: Vec<Rc<ChildRealm>> = {
                let mut live = self.live.borrow_mut();
                for child in live.iter() {
                    if child.unneeded() {
                        child.retired.set(true);
                    }
                }
                live.extract_if(.., |child| child.retired.get()).collect()
            };
            if freed.is_empty() {
                return released;
            }
            released = released.saturating_add(freed.len());
            debug_assert!(freed.iter().all(|child| Rc::strong_count(child) == 1));
            drop(freed);
        }
    }
}

impl ChildRealm {
    fn landed(&self) {
        self.in_flight.set(self.in_flight.get().saturating_sub(1));
    }

    fn unneeded(&self) -> bool {
        self.in_flight.get() == 0
            && !self.engine.is_referenced_across_realms()
            && self.timers.next_deadline().is_none()
            && !self.state.as_ref().is_some_and(|state| state.is_busy())
    }
}

pub(crate) trait ChildHostModules {
    fn allocate_opener(&self) -> OpenerId;

    fn install(
        &self,
        engine: &mut ScriptEngine,
        key: ChildKey,
        opener: OpenerId,
        retired: Rc<Cell<bool>>,
    ) -> Result<Box<dyn ChildHostState>, ScriptError>;
}

pub(crate) trait ChildHostState {
    fn is_busy(&self) -> bool;
}

pub(crate) struct ShadowRealmHook {
    pub(crate) children: Weak<ChildRealms>,
    pub(crate) host: HostOutbox,
    pub(crate) thread: JsThreadHandle,
    pub(crate) source: ScriptSource,
    pub(crate) opener: Opener,
    pub(crate) child_hosts: Option<Rc<dyn ChildHostModules>>,
    pub(crate) canvas_host: CanvasHost,
    #[cfg(feature = "devtools")]
    pub(crate) console: Rc<crate::devtools::runtime::ConsoleTap>,
}

pub(crate) fn install(engine: &mut ScriptEngine, hook: ShadowRealmHook) -> Result<(), ScriptError> {
    engine.install_shadow_realm(move |child| adopt(&hook, child))
}

fn adopt(hook: &ShadowRealmHook, mut engine: ScriptEngine) -> Result<(), String> {
    let children = hook
        .children
        .upgrade()
        .ok_or("the realm is being released")?;
    engine.enable_module_loading();
    let key = children.allocate();
    let retired = Rc::new(Cell::new(false));
    let mut state = None;
    let own_opener = hook
        .child_hosts
        .as_ref()
        .map(|modules| modules.allocate_opener());
    let installed = super::install_realm_core(
        &mut engine,
        &hook.host,
        hook.thread.clone(),
        FrameDemand::Ignored,
        RealmNames {
            source: hook.source.clone(),
            opener: own_opener.map_or(hook.opener, Opener::Realm),
            #[cfg(feature = "devtools")]
            console: Rc::clone(&hook.console),
        },
        &hook.children,
        hook.child_hosts.clone(),
        hook.canvas_host.clone(),
    )
    .and_then(|tables| {
        if let (Some(modules), Some(opener)) = (&hook.child_hosts, own_opener) {
            state = Some(modules.install(&mut engine, key, opener, Rc::clone(&retired))?);
        }
        Ok(tables)
    });
    let (tables, refused) = match installed {
        Ok(tables) => (tables, None),
        Err(error) => {
            retired.set(true);
            let unused = CoreTables {
                timers: Rc::new(TimerState::new()),
                futures: Rc::new(FutureTable::new()),
                canvases: Rc::new(CanvasTable::new(
                    &CanvasHost::Worker(std::sync::Arc::default()),
                    ReadWait::new(hook.host.token(), &hook.thread),
                    std::sync::Arc::clone(hook.host.canvas_hand_over()),
                )),
            };
            (unused, Some(error.message.to_string()))
        }
    };
    children.keep(ChildRealm {
        engine,
        key,
        timers: tables.timers,
        futures: tables.futures,
        canvases: tables.canvases,
        retired,
        in_flight: Cell::new(0),
        state,
    });
    refused.map_or(Ok(()), Err)
}
