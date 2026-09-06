//! A view's realm reaching its workers: the `Worker` object's whole host
//! surface, and the bookkeeping behind the keys it holds.
//!
//! The mirror of [`worker_scope`](super::worker_scope), which furnishes the
//! realm on the other side. Nothing here touches the document and nothing here
//! can: a worker's realm is on the group's other runtime, on the group's other
//! thread, and what crosses is a key and a JSON string.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;

use super::{
    MainThreadError, MainThreadRuntime, RUNTIME_MODULE_SPECIFIER, ScriptEngine, ScriptRuntime,
    ToPainterSender, install, number_argument, string_argument,
};
use crate::main::workers::{WorkerCommand, WorkerHub, WorkerKey, WorkerPayload, WorkerStart};
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
use crate::view::{EngineEvent, EventRequester, ToPainter, ViewId, WorkerScript};

/// Called on `bobcat:runtime`, in a view's realm, with one worker's news.
const WORKER_EVENT_EXPORT: &str = "__BobcatDeliverWorkerEvent";

/// One view's workers: what its realm has created, and what each is still
/// waiting for.
///
/// Shared with the host functions that maintain it, so it is `Rc` like
/// [`EventState`]: the native `createWorker` export and the command loop that
/// answers its fetch are different stack frames on the same thread.
///
/// The keys are also the boundary's whole validation: a realm may only name a
/// worker its own view created and has not ended, so a number a card invented
/// is a JavaScript exception rather than another view's worker.
pub(super) struct WorkerState<R: EventRequester> {
    view: ViewId,
    /// The group's worker thread and key sequence, shared with every other
    /// view on it.
    hub: Rc<WorkerHub>,
    /// Where a script request leaves for the thread that owns the fetcher.
    notify: ToPainterSender<R>,
    live: RefCell<FxHashMap<WorkerKey, WorkerSlot>>,
}

/// Where one worker is between `new Worker(...)` and its realm.
enum WorkerSlot {
    /// Constructed; its script is still being fetched.
    ///
    /// HTML queues what is posted before a worker's global scope exists and
    /// delivers it once the scope is up, which is what `queued` is: without
    /// it the commonest shape there is — construct, then post — would lose
    /// its first message.
    Loading { name: String, queued: Vec<String> },
    /// Its realm is up on the worker thread.
    Running,
}

impl<R: EventRequester> WorkerState<R> {
    pub(super) fn new(view: ViewId, hub: Rc<WorkerHub>, notify: ToPainterSender<R>) -> Self {
        Self {
            view,
            hub,
            notify,
            live: RefCell::default(),
        }
    }

    /// Names one worker and asks the painter for its script. The fetch is the
    /// painter's because the fetcher is: this thread never holds one.
    fn create(&self, url: &str, name: &str) -> WorkerKey {
        let key = self.hub.next_key();
        self.live.borrow_mut().insert(
            key,
            WorkerSlot::Loading {
                name: name.to_owned(),
                queued: Vec::new(),
            },
        );
        self.notify.send(ToPainter::RequestWorkerScript {
            key,
            url: url.to_owned(),
        });
        key
    }

    /// The script arrived. Starting the worker and flushing what was posted
    /// while it loaded are one step, so nothing can arrive out of order.
    fn started(&self, key: WorkerKey, script: WorkerScript) {
        let mut live = self.live.borrow_mut();
        // Terminated, or its view released, while the fetch was in flight.
        let Some(slot) = live.get_mut(&key) else {
            return;
        };
        let WorkerSlot::Loading { name, queued } = std::mem::replace(slot, WorkerSlot::Running)
        else {
            unreachable!("a script is answered once, for a worker that is still loading")
        };
        drop(live);
        let WorkerScript { source, url } = script;
        self.hub.start(Box::new(WorkerStart {
            key,
            view: self.view,
            name,
            url,
            source,
        }));
        for data in queued {
            self.hub.send(WorkerCommand::Message { key, data });
        }
    }

    /// A worker the realm may still name, or the reason it may not.
    fn validate(&self, function: &str, key: WorkerKey) -> Result<(), String> {
        if self.live.borrow().contains_key(&key) {
            Ok(())
        } else {
            Err(format!("{function} names no live worker of this view"))
        }
    }

    /// Forgets one worker: it ended, however it ended.
    fn forget(&self, key: WorkerKey) {
        self.live.borrow_mut().remove(&key);
    }

    /// Ends every worker this view created, because the view is gone.
    pub(super) fn release(&self) {
        let released = std::mem::take(&mut *self.live.borrow_mut());
        if !released.is_empty() {
            self.hub.send(WorkerCommand::ReleaseView(self.view));
        }
    }
}

impl<R: EventRequester> MainThreadRuntime<R> {
    /// The painter answered a `createWorker` request.
    ///
    /// A worker whose script could not be fetched or decoded is over before
    /// it began, and hears about it exactly the way one whose script threw
    /// does: an `error` event on the realm's `Worker`, and a
    /// [`EngineEvent::WorkerFailed`] for the embedder.
    pub(crate) fn worker_script_loaded(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        key: WorkerKey,
        script: Result<WorkerScript, String>,
    ) -> Result<(), MainThreadError> {
        match script {
            Ok(script) => {
                self.workers.started(key, script);
                Ok(())
            }
            Err(message) => self.deliver_worker_event(
                js_runtime,
                key,
                WorkerPayload::Failed(ScriptError {
                    kind: ScriptErrorKind::Other,
                    phase: ScriptErrorPhase::Execute,
                    message: Arc::from(format!("loading the worker's script: {message}")),
                    location: None,
                }),
            ),
        }
    }

    /// Hands the realm one thing a worker had to say.
    ///
    /// The failure paths report twice on purpose. The `error` event is the
    /// standard's own path and the only one a card can act on; the engine
    /// event is the only way an embedder learns a background script died,
    /// which it otherwise could not, because a card that registered no
    /// handler swallows the event entirely.
    pub(crate) fn deliver_worker_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        key: WorkerKey,
        payload: WorkerPayload,
    ) -> Result<(), MainThreadError> {
        let (kind, data) = match payload {
            WorkerPayload::Message(data) => ("message", data),
            WorkerPayload::Errored(error) => {
                let message = error.message.to_string();
                self.workers
                    .notify
                    .send(ToPainter::Engine(EngineEvent::WorkerFailed(error)));
                ("error", message)
            }
            WorkerPayload::Failed(error) => {
                let message = error.message.to_string();
                self.workers.forget(key);
                self.workers
                    .notify
                    .send(ToPainter::Engine(EngineEvent::WorkerFailed(error)));
                ("failed", message)
            }
            WorkerPayload::Closed => {
                self.workers.forget(key);
                ("closed", String::new())
            }
        };
        self.engine
            .call_module_export(
                js_runtime,
                RUNTIME_MODULE_SPECIFIER,
                WORKER_EVENT_EXPORT,
                &[
                    HostArgument::Number(key.as_number()),
                    HostArgument::String(kind),
                    HostArgument::String(&data),
                ],
            )
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine("delivering a worker event", error))
    }

    /// Ends every worker this view created. Called when the view is released:
    /// the realm is about to go, and nothing it started should outlive it.
    pub(crate) fn release_workers(&mut self) {
        self.workers.release();
    }
}

/// Installs the three members the realm's `Worker` speaks to.
///
/// None of them touches the document, and none of them can: a worker's realm
/// is on the group's other runtime, on the group's other thread, and what
/// crosses to it is a key and a JSON string.
pub(super) fn install_worker_host_members<R: EventRequester>(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    workers: &Rc<WorkerState<R>>,
) -> Result<(), MainThreadError> {
    let state = Rc::clone(workers);
    install(engine, js_runtime, "createWorker", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.createWorker";
        let url = string_argument(NAME, arguments, 0)?;
        let name = string_argument(NAME, arguments, 1)?;
        if url.is_empty() {
            return Err(format!("{NAME} requires a script URL"));
        }
        Ok(HostValue::Number(state.create(url, name).as_number()))
    })?;

    let state = Rc::clone(workers);
    install(
        engine,
        js_runtime,
        "postWorkerMessage",
        2,
        move |arguments| {
            const NAME: &str = "bobcat-internal:host.postWorkerMessage";
            let key = worker_key_argument(NAME, arguments, 0)?;
            let data = string_argument(NAME, arguments, 1)?;
            state.validate(NAME, key)?;
            let mut live = state.live.borrow_mut();
            match live.get_mut(&key) {
                // Still fetching: HTML queues what is posted before the
                // worker's global scope exists.
                Some(WorkerSlot::Loading { queued, .. }) => queued.push(data.to_owned()),
                Some(WorkerSlot::Running) => {
                    drop(live);
                    state.hub.send(WorkerCommand::Message {
                        key,
                        data: data.to_owned(),
                    });
                }
                None => unreachable!("the key was just validated as live"),
            }
            Ok(HostValue::Undefined)
        },
    )?;

    let state = Rc::clone(workers);
    install(engine, js_runtime, "terminateWorker", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.terminateWorker";
        let key = worker_key_argument(NAME, arguments, 0)?;
        state.validate(NAME, key)?;
        state.forget(key);
        // Cooperative: the worker thread drops the realm between tasks,
        // because nothing interrupts one mid-call. A worker still fetching
        // its script has no realm yet, and the answer finds no slot.
        state.hub.send(WorkerCommand::Terminate { key });
        Ok(HostValue::Undefined)
    })
}

/// A worker key, which the realm only ever passes back after the host handed
/// it one.
fn worker_key_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<WorkerKey, String> {
    let value = number_argument(function, arguments, index)?;
    WorkerKey::from_number(value)
        .ok_or_else(|| format!("{function} expects a worker key for argument {index}"))
}
