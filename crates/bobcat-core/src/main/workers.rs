//! The Worker handles of a view's main-thread realms and their message
//! channels.
//!
//! Every realm over the view's document has a [`WorkerOwner`] of its own: the
//! page realm, and each of its `ShadowRealm` children, which get the same
//! three `Worker` members over theirs. All of them share the view's
//! [`ViewWorkers`]: the one channel every worker of the view reports on, and
//! the table that says which child created which worker, so an event reaches
//! the realm whose `Worker` object it is for. A worker no child recorded is
//! the page realm's.
//!
//! JavaScript keeps weak references to Worker objects and releases a handle
//! through `terminateWorker` when the object is collected or explicitly
//! terminated. Each worker has its own cancellation scope. Releasing a view
//! does not cancel those scopes: MTS first completes its JS disposal protocol.
//! When a realm is released, its remaining senders close naturally: the page
//! realm's with the view, a child's with the child.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::quickjs::ScriptEngine;
use crate::background::{
    WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart,
};
use crate::esm::HOST_MODULE_SPECIFIER;
use crate::link::{ViewNotice, ViewOutbox};
use crate::realm::child::{ChildHostState, ChildKey};
use crate::script::ScriptError;
use crate::threads::platform_script_error;
use crate::view::{ScriptSource, WorkerId};

/// Issued on bobcat-main, once per group. No cross-thread allocator or lock:
/// the one thing it reads across threads is the worker thread's trap flag.
#[derive(Clone)]
pub(crate) struct WorkerFactory {
    commands: mpsc::UnboundedSender<WorkerCommand>,
    /// Set by `bobcat-workers` once it has trapped, after which a `Start`
    /// would be sent to a thread that never reads it.
    trapped: Arc<AtomicBool>,
    next: Rc<Cell<u64>>,
}

impl WorkerFactory {
    pub(crate) fn new(
        commands: mpsc::UnboundedSender<WorkerCommand>,
        trapped: Arc<AtomicBool>,
    ) -> Self {
        Self {
            commands,
            trapped,
            next: Rc::new(Cell::new(1)),
        }
    }

    /// One view's side of its workers, and the channel everything they say
    /// arrives on — every realm's workers, the page realm's and its
    /// children's alike.
    pub(super) fn view(
        &self,
        outbox: ViewOutbox,
    ) -> (Rc<ViewWorkers>, mpsc::UnboundedReceiver<WorkerEvent>) {
        let (events, incoming) = mpsc::unbounded_channel();
        let view = Rc::new(ViewWorkers {
            factory: self.clone(),
            outbox,
            events,
            routes: RefCell::default(),
        });
        (view, incoming)
    }
}

/// What every realm of one view shares about its workers.
///
/// Held strongly by each realm's [`WorkerOwner`] alone, so it goes with the
/// last of them: the page realm's, which its runtime drops after its realm
/// and its children.
pub(super) struct ViewWorkers {
    factory: WorkerFactory,
    /// The view's own, which every worker's `HostOutbox` is made from and
    /// every worker failure is reported through.
    outbox: ViewOutbox,
    /// Cloned into every `Start`, so the channel stays open while a realm
    /// does even when it has no worker at all.
    events: mpsc::UnboundedSender<WorkerEvent>,
    /// The child realm that created each worker a child created, by the
    /// child's own owner: an entry from the key's allocation until the child
    /// lets go of the key (`terminate()`, delivery of the worker's own end)
    /// or is freed. A key with no entry is the page realm's.
    ///
    /// Weak, so this table keeps no child's workers alive: the child's
    /// owner, which the child holds, is what does.
    routes: RefCell<FxHashMap<WorkerKey, Weak<WorkerOwner>>>,
}

impl ViewWorkers {
    /// A fresh owner for one realm's workers: the page realm's under `None`,
    /// a `ShadowRealm` child's under its key.
    pub(super) fn owner(self: &Rc<Self>, realm: Option<ChildKey>) -> Rc<WorkerOwner> {
        Rc::new(WorkerOwner {
            view: Rc::clone(self),
            realm,
            live: RefCell::default(),
            sources: RefCell::default(),
        })
    }

    /// The owner of the child realm that created the worker under `key`, if
    /// a child did; `None` is the page realm's, or a key no realm holds any
    /// more, which the page realm's own records then name nothing for.
    pub(super) fn child_owner(&self, key: WorkerKey) -> Option<Rc<WorkerOwner>> {
        self.routes.borrow().get(&key).and_then(Weak::upgrade)
    }
}

/// One realm's whole side of its workers.
pub(super) struct WorkerOwner {
    view: Rc<ViewWorkers>,
    /// The realm these workers are delivered to: `None` for the page realm,
    /// the child's key for a `ShadowRealm` child, whose keys this owner
    /// records in the view's routes.
    realm: Option<ChildKey>,
    /// One sender per live JS handle. Explicit termination or its finalizer
    /// removes it; a worker which closed itself is forgotten on delivery.
    /// Dropping this map closes the remaining channels without a stop sweep.
    live: RefCell<FxHashMap<WorkerKey, mpsc::UnboundedSender<WorkerMessage>>>,
    /// The source of each worker whose key the script still holds. Kept
    /// apart from `live`: an entry is made when the key is allocated, before
    /// the `Start` is sent, so a worker that failed at once and never entered
    /// `live` has one too. It is removed where the script lets go of the key:
    /// `terminate()`, or delivery of the worker's own end.
    sources: RefCell<FxHashMap<WorkerKey, ScriptSource>>,
}

impl WorkerOwner {
    /// Installs the three members a realm creates and drives workers through,
    /// over this owner.
    ///
    /// The owner is held by whatever owns the realm — the page's runtime, or
    /// the child. Host functions borrow it weakly: cleanup jobs queued while
    /// a realm is released must not keep the owner, its channels, or the
    /// group thread alive.
    pub(super) fn install(self: &Rc<Self>, engine: &mut ScriptEngine) -> Result<(), ScriptError> {
        let creator = Rc::downgrade(self);
        engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            "createWorker",
            3,
            Box::new(move |arguments| {
                let creator = creator
                    .upgrade()
                    .ok_or("the creating realm has been released")?;
                let specifier = string(arguments, 0)?;
                let name = string(arguments, 1)?.to_owned();
                // Where the specifier resolves from: `__Card__`, which the
                // realm hands over with every construction — the view's card
                // URL (a container page's container URL), or else the MTS
                // entry's response URL; in a frame's realm, the frame's
                // card, which `bobcat:frame` resolved against the view's base
                // before it booted the card. It is absolute: a card URL is
                // resolved at the view's construction, and an entry answered
                // from a response URL that is not fails the view's startup
                // before it runs.
                // Rust joins the specifier to it by URL rules and still does
                // not remember it. An absolute URL joins to itself,
                // `bobcat:bts` included. A specifier that does not resolve
                // starts nothing: no id, no `Start`, and `null` back, which
                // the realm throws as a `SyntaxError`.
                let base_url = string(arguments, 2)?;
                let Ok(url) = url::Url::parse(base_url).and_then(|base| base.join(specifier))
                else {
                    return Ok(HostValue::Null);
                };
                let url = String::from(url);
                let factory = &creator.view.factory;
                let id = factory.next.get();
                factory
                    .next
                    .set(id.checked_add(1).ok_or("worker ids exhausted")?);
                let key = WorkerKey::new(id);
                // Each worker's own end signal, which every request it makes
                // of the host is cancelled with as well. The host is asked
                // for nothing here: the worker's realm loads the module at
                // `url` itself, and asks through `sources` below when that
                // load needs the host.
                let token = CancellationToken::new();
                let (messages, incoming) = mpsc::unbounded_channel();
                let start = WorkerStart {
                    key,
                    name,
                    url,
                    messages: incoming,
                    events: creator.view.events.clone(),
                    sources: creator.view.outbox.host_outbox(token.clone()),
                    token,
                };
                creator.start(start, messages);
                Ok(HostValue::String(id.to_string()))
            }),
        )?;
        let sender = Rc::downgrade(self);
        engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            "sendWorkerMessage",
            2,
            Box::new(move |arguments| {
                let key = key(arguments)?;
                // Any value the boundary carries. A value the serializer
                // refuses throws in the realm that called `postMessage`, so
                // nothing here has to decide what a message may be.
                let data = arguments
                    .get(1)
                    .map_or(HostValue::Undefined, |data| data.to_value());
                if let Some(sender) = sender.upgrade() {
                    sender.post(key, WorkerMessage::Post(data));
                }
                Ok(HostValue::Undefined)
            }),
        )?;
        let terminator = Rc::downgrade(self);
        engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            "terminateWorker",
            1,
            Box::new(move |arguments| {
                let key = key(arguments)?;
                if let Some(terminator) = terminator.upgrade() {
                    terminator.terminate(key);
                }
                Ok(HostValue::Undefined)
            }),
        )?;
        Ok(())
    }

    /// The child realm these workers are delivered to, or `None` for the
    /// page realm.
    pub(super) const fn realm(&self) -> Option<ChildKey> {
        self.realm
    }

    /// The owner of the child realm of this view that created the worker
    /// under `key`, if a child did: see [`ViewWorkers::child_owner`].
    pub(super) fn child_owner(&self, key: WorkerKey) -> Option<Rc<Self>> {
        self.view.child_owner(key)
    }

    /// Sends `start` to `bobcat-workers` and keeps `messages`, the sending
    /// end of its message channel, while the worker runs.
    ///
    /// The worker's source is recorded before anything is sent, so a worker
    /// that fails at once has one too, and so is its route, for a child's:
    /// that failure is the child's to hear. The source is the id of the
    /// worker's key and the worker's URL, whatever that URL is, and the
    /// worker thread writes the same value from the same `Start` when it
    /// opens the realm, so the `Start` carries no source. Neither a Worker
    /// nor a request it makes of the host inherits the view's cancellation
    /// token: both end with the token `start` carries.
    ///
    /// A worker thread that has trapped, or one whose inbox is closed, fails
    /// the worker at once. A worker that has already failed is still a
    /// worker to the script that named it: its `Failed` is queued on the
    /// view's channel and reaches the script as an `error` event. Nothing is
    /// sent to the thread or kept in `live`.
    fn start(self: &Rc<Self>, start: WorkerStart, messages: mpsc::UnboundedSender<WorkerMessage>) {
        let key = start.key;
        self.sources.borrow_mut().insert(
            key,
            ScriptSource::Worker {
                id: WorkerId::from(key),
                url: Arc::from(start.url.as_str()),
            },
        );
        if self.realm.is_some() {
            self.view
                .routes
                .borrow_mut()
                .insert(key, Rc::downgrade(self));
        }
        let factory = &self.view.factory;
        if !factory.trapped.load(Ordering::Acquire) {
            // The view's own outbox, whichever realm created the worker, so
            // the view can reach every worker of its own — a frame's card's
            // background thread answering a native module call included.
            self.view.outbox.notify(ViewNotice::WorkerCreated {
                key,
                messages: messages.downgrade(),
            });
            let started = factory.commands.send(WorkerCommand::Start(start));
            // On a refused send `messages` drops at the end of this block, so
            // the handle `WorkerCreated` registered no longer upgrades and the
            // view sweeps it.
            if started.is_ok() {
                self.live.borrow_mut().insert(key, messages);
                return;
            }
        }
        let _ = self.view.events.send(WorkerEvent {
            key,
            payload: WorkerPayload::Failed(platform_script_error(
                "the worker thread has ended".to_owned(),
            )),
        });
    }

    fn post(&self, key: WorkerKey, message: WorkerMessage) {
        if let Some(messages) = self.live.borrow().get(&key) {
            let _ = messages.send(message);
        }
    }

    /// `Worker.terminate()`: the worker takes nothing more, including what is
    /// already queued for it.
    ///
    /// A message rather than simply dropping the sender, because dropping is
    /// the *other* thing: a closed channel is read to its end, so everything
    /// queued would be delivered first. This is read in order like any other
    /// message, and reading it ends the worker at once — which discards the
    /// deliveries queued ahead of it that have not run, the way HTML's
    /// "terminate a worker" discards its queued tasks.
    ///
    /// Its source and its route go too: the script has let go of the key, so
    /// whatever the worker says after this is dropped before it reaches a
    /// `Worker` object, and a `terminate()` is reported to no one.
    fn terminate(&self, key: WorkerKey) {
        self.let_go(key);
        if let Some(messages) = self.live.borrow_mut().remove(&key) {
            let _ = messages.send(WorkerMessage::Terminate);
        }
    }

    /// Tells the embedder that the worker `source` names threw, or, when
    /// `ended`, that it ended without being told to.
    ///
    /// Reported from here because this is the realm's side of its workers:
    /// `WorkerThrew` and `WorkerEnded` are the only lifecycle events a worker
    /// produces, and the outbox they go out on is the view's own, which this
    /// side already holds: every worker's `HostOutbox` is made from it.
    pub(super) fn report_failure(&self, source: ScriptSource, ended: bool, error: ScriptError) {
        self.view.outbox.engine_event(if ended {
            crate::EngineEvent::WorkerEnded { source, error }
        } else {
            crate::EngineEvent::WorkerThrew { source, error }
        });
    }

    /// Drops what this side kept of a worker that ended on its own — it
    /// called `close()`, its realm could not be built, or its thread trapped
    /// — so `live` goes on naming only the workers still running.
    ///
    /// The sender goes with the entry, which closes that channel. Harmless
    /// either way: there is nothing left listening on it.
    pub(super) fn forget(&self, key: WorkerKey) {
        self.live.borrow_mut().remove(&key);
        self.let_go(key);
    }

    /// Drops the source of a key the script let go of, and its route.
    fn let_go(&self, key: WorkerKey) {
        if self.sources.borrow_mut().remove(&key).is_some() && self.realm.is_some() {
            self.view.routes.borrow_mut().remove(&key);
        }
    }

    /// Which realm the worker under `key` is, while the script still holds
    /// that key.
    pub(super) fn source_of(&self, key: WorkerKey) -> Option<ScriptSource> {
        self.sources.borrow().get(&key).cloned()
    }

    /// How many workers this realm still has running.
    pub(super) fn live_workers(&self) -> usize {
        self.live.borrow().len()
    }
}

/// A child's workers keep the child: see [`ChildHostState::is_busy`].
impl ChildHostState for Rc<WorkerOwner> {
    fn is_busy(&self) -> bool {
        self.live_workers() > 0
    }
}

/// A child's owner goes with the child, after its realm: its senders close,
/// which ends the workers the child still had running, and its keys leave
/// the routes, so what those workers said last reaches no realm.
impl Drop for WorkerOwner {
    fn drop(&mut self) {
        if self.realm.is_none() {
            return;
        }
        let mut routes = self.view.routes.borrow_mut();
        for key in self.sources.get_mut().keys() {
            routes.remove(key);
        }
    }
}

fn string<'a>(arguments: &[HostArgument<'a>], index: usize) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(&HostArgument::String(value)) => Ok(value),
        _ => Err(format!(
            "Worker host operation expects string argument {index}"
        )),
    }
}

fn key(arguments: &[HostArgument<'_>]) -> Result<WorkerKey, String> {
    string(arguments, 0)?
        .parse()
        .map(WorkerKey::new)
        .map_err(|_| "invalid worker key".to_owned())
}
