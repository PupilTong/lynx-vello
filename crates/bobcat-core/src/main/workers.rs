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

#[derive(Clone)]
pub(crate) struct WorkerFactory {
    commands: mpsc::UnboundedSender<WorkerCommand>,
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
            #[cfg(feature = "devtools")]
            devtools: RefCell::default(),
        });
        (view, incoming)
    }
}

pub(super) struct ViewWorkers {
    factory: WorkerFactory,
    outbox: ViewOutbox,
    events: mpsc::UnboundedSender<WorkerEvent>,
    routes: RefCell<FxHashMap<WorkerKey, Weak<WorkerOwner>>>,
    #[cfg(feature = "devtools")]
    devtools: RefCell<Option<crate::devtools::target::AutoAttach>>,
}

impl ViewWorkers {
    pub(super) fn owner(self: &Rc<Self>, realm: Option<ChildKey>) -> Rc<WorkerOwner> {
        Rc::new(WorkerOwner {
            view: Rc::clone(self),
            realm,
            live: RefCell::default(),
            sources: RefCell::default(),
        })
    }

    pub(super) fn child_owner(&self, key: WorkerKey) -> Option<Rc<WorkerOwner>> {
        self.routes.borrow().get(&key).and_then(Weak::upgrade)
    }
}

pub(super) struct WorkerOwner {
    view: Rc<ViewWorkers>,
    realm: Option<ChildKey>,
    live: RefCell<FxHashMap<WorkerKey, mpsc::UnboundedSender<WorkerMessage>>>,
    sources: RefCell<FxHashMap<WorkerKey, ScriptSource>>,
}

impl WorkerOwner {
    pub(super) fn install(self: &Rc<Self>, engine: &mut ScriptEngine) -> Result<(), ScriptError> {
        let creator = Rc::downgrade(self);
        engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            "createWorker",
            3,
            move |arguments| {
                let creator = creator
                    .upgrade()
                    .ok_or("the creating realm has been released")?;
                let specifier = string(arguments, 0)?;
                let name = string(arguments, 1)?.to_owned();
                let base_url = string(arguments, 2)?;
                let Ok(url) = url::Url::parse(base_url).and_then(|base| base.join(specifier))
                else {
                    return Ok(HostValue::Null);
                };
                let url = String::from(url);
                let factory = &creator.view.factory;
                let id = factory.next.get();
                if id > MAX_WORKER_KEY {
                    return Err("worker ids exhausted".to_owned());
                }
                factory.next.set(id + 1);
                let key = WorkerKey::new(id);
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
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "at most `MAX_WORKER_KEY`, which an f64 holds exactly"
                )]
                Ok(HostValue::Number(id as f64))
            },
        )?;
        let sender = Rc::downgrade(self);
        engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            "sendWorkerMessage",
            2,
            move |arguments| {
                let key = key(arguments)?;
                let data = arguments
                    .get(1)
                    .map_or(HostValue::Undefined, |data| data.to_value());
                if let Some(sender) = sender.upgrade() {
                    let enqueued = sender.view.outbox.registry().js_blocking_stamp();
                    sender.post(key, WorkerMessage::Post { data, enqueued });
                }
                Ok(HostValue::Undefined)
            },
        )?;
        let terminator = Rc::downgrade(self);
        engine.register_host_module_function(
            HOST_MODULE_SPECIFIER,
            "terminateWorker",
            1,
            move |arguments| {
                let key = key(arguments)?;
                if let Some(terminator) = terminator.upgrade() {
                    terminator.terminate(key);
                }
                Ok(HostValue::Undefined)
            },
        )?;
        Ok(())
    }

    pub(super) const fn realm(&self) -> Option<ChildKey> {
        self.realm
    }

    pub(super) fn child_owner(&self, key: WorkerKey) -> Option<Rc<Self>> {
        self.view.child_owner(key)
    }

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
            self.view.outbox.notify(ViewNotice::WorkerCreated {
                key,
                messages: messages.downgrade(),
            });
            let started = factory.commands.send(WorkerCommand::Start(start));
            if started.is_ok() {
                #[cfg(feature = "devtools")]
                self.devtools_started(key, &messages);
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

    pub(super) fn post(&self, key: WorkerKey, message: WorkerMessage) {
        if let Some(messages) = self.live.borrow().get(&key) {
            let _ = messages.send(message);
        }
    }

    fn terminate(&self, key: WorkerKey) {
        #[cfg(feature = "devtools")]
        self.devtools_ended(key);
        self.let_go(key);
        if let Some(messages) = self.live.borrow_mut().remove(&key) {
            let _ = messages.send(WorkerMessage::Terminate);
        }
    }

    pub(super) fn report_failure(&self, source: ScriptSource, ended: bool, error: ScriptError) {
        self.view.outbox.engine_event(if ended {
            crate::EngineEvent::WorkerEnded { source, error }
        } else {
            crate::EngineEvent::WorkerThrew { source, error }
        });
    }

    pub(super) fn forget(&self, key: WorkerKey) {
        #[cfg(feature = "devtools")]
        self.devtools_ended(key);
        self.live.borrow_mut().remove(&key);
        self.let_go(key);
    }

    fn let_go(&self, key: WorkerKey) {
        if self.sources.borrow_mut().remove(&key).is_some() && self.realm.is_some() {
            self.view.routes.borrow_mut().remove(&key);
        }
    }

    pub(super) fn source_of(&self, key: WorkerKey) -> Option<ScriptSource> {
        self.sources.borrow().get(&key).cloned()
    }

    pub(super) fn live_workers(&self) -> usize {
        self.live.borrow().len()
    }
}

#[cfg(feature = "devtools")]
impl WorkerOwner {
    pub(super) fn devtools_auto_attach(
        &self,
    ) -> &RefCell<Option<crate::devtools::target::AutoAttach>> {
        &self.view.devtools
    }

    pub(super) fn devtools_live_workers(&self) -> Vec<crate::devtools::target::LiveWorker> {
        let mut workers = Vec::new();
        let mut add = |owner: &Self, key: WorkerKey| {
            if let (Some(messages), Some(ScriptSource::Worker { url, .. })) = (
                owner.live.borrow().get(&key),
                owner.sources.borrow().get(&key),
            ) {
                workers.push((key, Arc::clone(url), messages.clone()));
            }
        };
        for &key in self.live.borrow().keys() {
            add(self, key);
        }
        for (&key, owner) in self.view.routes.borrow().iter() {
            if let Some(owner) = owner.upgrade() {
                add(&owner, key);
            }
        }
        workers.sort_unstable_by_key(|(key, ..)| *key);
        workers
    }

    fn devtools_started(&self, key: WorkerKey, messages: &mpsc::UnboundedSender<WorkerMessage>) {
        if let Some(attached) = self.view.devtools.borrow().as_ref()
            && let Some(ScriptSource::Worker { url, .. }) = self.sources.borrow().get(&key)
        {
            attached.worker_started(key, url, messages);
        }
    }

    fn devtools_ended(&self, key: WorkerKey) {
        if let Some(attached) = self.view.devtools.borrow().as_ref() {
            attached.worker_ended(key);
        }
    }
}

impl ChildHostState for Rc<WorkerOwner> {
    fn is_busy(&self) -> bool {
        self.live_workers() > 0
    }
}

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

const MAX_WORKER_KEY: u64 = 1 << 53;

fn key(arguments: &[HostArgument<'_>]) -> Result<WorkerKey, String> {
    let Some(&HostArgument::Number(value)) = arguments.first() else {
        return Err("Worker host operation expects number argument 0".to_owned());
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "2^53, which an f64 holds exactly"
    )]
    let max = MAX_WORKER_KEY as f64;
    if value >= 1.0 && value.fract() == 0.0 && value <= max {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "an integer in 1..=2^53, checked above"
        )]
        Ok(WorkerKey::new(value as u64))
    } else {
        Err("invalid worker key".to_owned())
    }
}
