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
//! Every operation on one child detaches it from the list first and puts it
//! back after, so a `new ShadowRealm()` the child's code makes pushes a
//! grandchild without meeting a borrow of the list.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use quickjs_rust_bridge::HostArgument;

use super::FrameDemand;
use crate::clock::ClockInstant;
use crate::future::{FutureTable, HostFuture, Outcome};
use crate::jobs::JsThreadHandle;
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;
use crate::timers::TimerState;
use crate::view::ScriptSource;

/// Which child of an owner's realm a module load or a future settle is for,
/// handed out in construction order and never reused, so a load that arrives
/// for a released child finds nothing rather than a newer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ChildKey(u64);

impl ChildKey {
    /// The key's own number, which [`super::RealmId::listener_key`] files a
    /// child under.
    pub(super) fn number(self) -> u64 {
        self.0
    }

    /// The key whose [`Self::number`] is `number`.
    pub(super) fn from_number(number: u64) -> Self {
        Self(number)
    }
}

/// One `ShadowRealm` child: its realm, the two tables its core members
/// write to, and what its owner's host modules keep for it.
pub(crate) struct ChildRealm {
    /// Declared first, so the realm — and every host function holding a
    /// clone of either table, or a handle on `state` — goes before the
    /// tables and the state do.
    engine: ScriptEngine,
    key: ChildKey,
    timers: Rc<TimerState>,
    futures: Rc<FutureTable>,
    /// Set once the child is to be released; the epilogue's last step drops
    /// it. Shared, so that whatever retires a child can say so without
    /// borrowing the list.
    retired: Rc<Cell<bool>>,
    /// The module loads and future settles spawned for this child and not
    /// yet delivered: a child waiting on one is still needed, whoever refers
    /// to it.
    in_flight: Cell<u32>,
    /// What the owner's [`ChildHostModules`] keep for this child — an MTS
    /// child's workers — or `None` for an owner that names none and for a
    /// child whose furnishing failed. Dropped after `engine`.
    state: Option<Box<dyn ChildHostState>>,
}

/// Every `ShadowRealm` child of one owner's realm, grandchildren included,
/// in construction order.
pub(crate) struct ChildRealms {
    live: RefCell<Vec<ChildRealm>>,
    next: Cell<u64>,
}

impl ChildRealms {
    pub(crate) fn new() -> Self {
        Self {
            live: RefCell::new(Vec::new()),
            // From 1: 0 is the owner's own realm's number
            // ([`super::RealmId::listener_key`]).
            next: Cell::new(1),
        }
    }

    /// How many children are live, for a test.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.live.borrow().len()
    }

    /// Whether this realm has no child to service, which is what decides
    /// whether the owner's own rejections are read beside theirs.
    pub(crate) fn is_empty(&self) -> bool {
        self.live.borrow().is_empty()
    }

    /// The key the next child is kept under, handed out before the child is
    /// furnished, so that what its host modules record of it names it.
    fn allocate(&self) -> ChildKey {
        // Sixty-four bits do not run out, so a key is never reused.
        let id = self.next.get();
        self.next.set(id.saturating_add(1));
        ChildKey(id)
    }

    /// Every live child's key, in construction order.
    pub(crate) fn keys(&self) -> Vec<ChildKey> {
        self.live.borrow().iter().map(|child| child.key).collect()
    }

    /// Runs `operation` on the child under `key`, detached from the list for
    /// the length of the call; `None` is a child that is gone.
    ///
    /// The child goes back afterwards, unwinding included: only the
    /// epilogue's release step frees a child.
    fn with_child<T>(
        &self,
        key: ChildKey,
        operation: impl FnOnce(&mut ChildRealm) -> T,
    ) -> Option<T> {
        let child = {
            let mut live = self.live.borrow_mut();
            let index = live.binary_search_by_key(&key, |child| child.key).ok()?;
            live.remove(index)
        };
        let mut detached = Detached {
            list: self,
            child: Some(child),
        };
        detached.child.as_mut().map(operation)
    }

    /// Puts a child where its key sorts: a detached one back, or a new one
    /// [`adopt`] furnished, under the key [`Self::allocate`] gave it.
    fn reattach(&self, child: ChildRealm) {
        let mut live = self.live.borrow_mut();
        let index = live
            .binary_search_by_key(&child.key, |live| live.key)
            .unwrap_or_else(|index| index);
        live.insert(index, child);
    }

    /// Runs every child's due timers, in construction order, and answers
    /// with what their callbacks threw; `None` when no child had one due.
    pub(crate) fn run_due_timers(&self, js: &mut ScriptRuntime) -> Option<Vec<ScriptError>> {
        let mut failures: Option<Vec<ScriptError>> = None;
        for key in self.keys() {
            let ran = self.with_child(key, |child| {
                crate::timers::run_due_timers(&mut child.engine, js, &child.timers)
            });
            if let Some(Some(thrown)) = ran {
                failures.get_or_insert_with(Vec::new).extend(thrown);
            }
        }
        failures
    }

    /// Every module request a child's code left, each counted as in flight
    /// for its child until [`Self::complete_module`] delivers it.
    pub(crate) fn take_module_requests(&self) -> Vec<(ChildKey, String)> {
        let mut requests = Vec::new();
        for child in self.live.borrow_mut().iter_mut() {
            while let Some(url) = child.engine.take_module_request() {
                child.in_flight.set(child.in_flight.get().saturating_add(1));
                requests.push((child.key, url));
            }
        }
        requests
    }

    /// Every future a `.then` in a child asked to settle, each counted as in
    /// flight for its child until [`Self::deliver_future`] delivers it.
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

    /// Completes one module request of the child under `key`; `None` is a
    /// child that is gone, which drops the answer.
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

    /// Hands one settled future to the child under `key`; `None` is a child
    /// that is gone, which drops the outcome.
    pub(crate) fn deliver_future(
        &self,
        js: &mut ScriptRuntime,
        key: ChildKey,
        id: u32,
        outcome: Outcome,
    ) -> Option<Result<(), ScriptError>> {
        self.with_child(key, |child| {
            child.landed();
            crate::future::deliver(&mut child.engine, js, id, outcome)
        })
    }

    /// Calls an export of a module the child under `key` has loaded, the way
    /// [`ScriptEngine::call_module_export`] calls one of the owner's: the
    /// checkpoint that ends the call is the child's, so what the call left
    /// unhandled is the child's failure. `None` is a child that is gone,
    /// which drops the call.
    ///
    /// For what the host hands one particular child rather than its owner —
    /// an event of a worker that child created.
    pub(crate) fn call_module_export(
        &self,
        js: &mut ScriptRuntime,
        key: ChildKey,
        module_specifier: &str,
        export_name: &str,
        arguments: &[HostArgument<'_>],
    ) -> Option<Result<bool, ScriptError>> {
        self.with_child(key, |child| {
            child
                .engine
                .call_module_export(js, module_specifier, export_name, arguments)
        })
    }

    /// One rejection per child that left any unhandled, in construction
    /// order: the entry that ran the child's code checkpointed for its own
    /// realm, which leaves a child's rejections queued for the child.
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

    /// Drops every rejection the children left unhandled, for an entry of
    /// their owner that has already reported its failure. No JavaScript
    /// runs.
    pub(crate) fn discard_rejections(&self, js: &mut ScriptRuntime) {
        // A child that is detached is the one running, and its own entry
        // decides about its rejections.
        let Ok(live) = self.live.try_borrow() else {
            return;
        };
        for child in live.iter() {
            child.engine.discard_own_rejections(js);
        }
    }

    /// The earliest timer deadline any child has armed.
    pub(crate) fn next_deadline(&self) -> Option<ClockInstant> {
        self.live
            .borrow()
            .iter()
            .filter_map(|child| child.timers.next_deadline())
            .min()
    }

    /// Frees every child nothing needs any more: retired, or referred to by
    /// nothing of another realm with no timer armed, nothing in flight and
    /// nothing its host state keeps it for. Answers with how many were freed.
    ///
    /// Run by the epilogue, and by the MTS realm right after it hands a
    /// `frame`'s disconnection to `bobcat:frame`, so the child that disposal
    /// retired goes in that entry; always outside every JavaScript call:
    /// freeing a realm collects the whole runtime. One child's release can leave
    /// another unreferenced — a grandchild only its `ShadowRealm` held — so
    /// this repeats until nothing more goes.
    pub(crate) fn release_unneeded(&self) -> usize {
        let mut released = 0usize;
        loop {
            let freed: Vec<ChildRealm> = {
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
            // Outside the borrow: freeing a context runs finalizers, and
            // nothing they reach may find the list borrowed.
            drop(freed);
        }
    }
}

impl ChildRealm {
    /// One load or settle for this child has been delivered.
    fn landed(&self) {
        self.in_flight.set(self.in_flight.get().saturating_sub(1));
    }

    /// Whether nothing needs this child any more. A retired child is freed
    /// whatever this answers.
    fn unneeded(&self) -> bool {
        self.in_flight.get() == 0
            && !self.engine.is_referenced_across_realms()
            && self.timers.next_deadline().is_none()
            && !self.state.as_ref().is_some_and(|state| state.is_busy())
    }
}

/// A child taken out of the list for one operation, put back on drop.
struct Detached<'a> {
    list: &'a ChildRealms,
    child: Option<ChildRealm>,
}

impl Drop for Detached<'_> {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            self.list.reattach(child);
        }
    }
}

/// The host modules an owner's `ShadowRealm` children get beyond the core,
/// installed into every child — grandchildren included — right after it.
///
/// `key` is the key the child will be kept under, allocated before this
/// runs, so that whatever the modules record about the child names it.
/// `retired` is the child's own release flag, the one [`ChildRealms`] keeps
/// it under: setting it is how a member of the child's says the child is to
/// be freed by the next epilogue's release step, whoever still refers to it.
///
/// What it answers is what the modules keep for this child, which the child
/// holds and drops with itself, after its realm.
///
/// Whatever implements this is held by every realm's hook for the owner's
/// life, so it must hold nothing that holds an engine of the owner's.
pub(crate) trait ChildHostModules {
    fn install(
        &self,
        engine: &mut ScriptEngine,
        key: ChildKey,
        retired: Rc<Cell<bool>>,
    ) -> Result<Box<dyn ChildHostState>, ScriptError>;
}

/// What an owner's [`ChildHostModules`] keep for one child, for as long as
/// the child lives: held by the child, and dropped with it, after its realm,
/// so whatever it holds ends with the child.
pub(crate) trait ChildHostState {
    /// Whether the child is still needed for this state's sake, whoever
    /// refers to it. An MTS child with a worker of its own still running is:
    /// what that worker says is delivered to the child, and freeing the child
    /// would end the worker in the middle of whatever it was answering — a
    /// card's background thread acknowledging its disposal. A retired child
    /// is freed all the same.
    fn is_busy(&self) -> bool;
}

/// What every `new ShadowRealm()` in one owner's realms is furnished from:
/// the list it joins, what its core members reach the host through, and the
/// owner's [`ChildHostModules`], if it has any.
///
/// The list is held weakly: a strong one would be a cycle no drop ends — the
/// list holds the child's engine, the engine holds this hook, and the hook
/// would hold the list.
pub(crate) struct ShadowRealmHook {
    pub(crate) children: Weak<ChildRealms>,
    pub(crate) host: HostOutbox,
    pub(crate) thread: JsThreadHandle,
    pub(crate) source: ScriptSource,
    pub(crate) child_hosts: Option<Rc<dyn ChildHostModules>>,
}

/// Installs the `ShadowRealm` constructor on `engine`, every construction
/// furnished by `hook`.
pub(crate) fn install(engine: &mut ScriptEngine, hook: ShadowRealmHook) -> Result<(), ScriptError> {
    engine.install_shadow_realm(move |child| adopt(&hook, child))
}

/// Furnishes one fresh child and keeps it: its key, module loading, then the
/// realm core with its own tables and the `ShadowRealm` constructor of its
/// own, then the owner's [`ChildHostModules`] over the child's key and
/// `retired` flag, then the list.
///
/// A child whose furnishing failed is kept retired and refused: the
/// construction throws, and the release step frees the realm. Only a list
/// that is already gone — an owner whose realm is being released, where no
/// JavaScript runs — lets the child go here.
fn adopt(hook: &ShadowRealmHook, mut engine: ScriptEngine) -> Result<(), String> {
    let children = hook
        .children
        .upgrade()
        .ok_or("the realm is being released")?;
    engine.enable_module_loading();
    // Before the modules are installed, which record what the child starts
    // under it. Nothing between here and the list runs JavaScript, so no
    // other child is kept in between.
    let key = children.allocate();
    // First, so the members that retire the child and the list that frees
    // it read one flag.
    let retired = Rc::new(Cell::new(false));
    let mut state = None;
    let installed = super::install_realm_core(
        &mut engine,
        &hook.host,
        hook.thread.clone(),
        FrameDemand::Ignored,
        hook.source.clone(),
        &hook.children,
        hook.child_hosts.clone(),
    )
    .and_then(|tables| {
        if let Some(modules) = &hook.child_hosts {
            state = Some(modules.install(&mut engine, key, Rc::clone(&retired))?);
        }
        Ok(tables)
    });
    // A child the hook could not furnish is kept all the same, retired:
    // the hook runs inside a JavaScript call, where no realm is freed, and
    // the epilogue's release step frees it.
    let (timers, futures, refused) = match installed {
        Ok((timers, futures)) => (timers, futures, None),
        Err(error) => {
            retired.set(true);
            let unused = (Rc::new(TimerState::new()), Rc::new(FutureTable::new()));
            (unused.0, unused.1, Some(error.message.to_string()))
        }
    };
    children.reattach(ChildRealm {
        engine,
        key,
        timers,
        futures,
        retired,
        in_flight: Cell::new(0),
        state,
    });
    refused.map_or(Ok(()), Err)
}
