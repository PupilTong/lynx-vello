//! A view's realm reaching its workers: the `Worker` object's whole host
//! surface, and nothing more than that.
//!
//! The realms those objects stand for are the group's, on the group's other
//! runtime, on the group's other thread — [`crate::background`]. What is here
//! is only what has to be on *this* thread: the three members a realm calls,
//! the keys it is allowed to name, and the delivery of what a worker said back
//! into the realm that can hear it.
//!
//! Nothing here touches the document and nothing here can: what crosses to a
//! worker is a key and a JSON string.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashSet;

use super::{
    MainThreadError, MainThreadRuntime, RUNTIME_MODULE_SPECIFIER, ScriptEngine, ScriptRuntime,
    ToPainterSender, install, string_argument,
};
use crate::background::{WorkerCommand, WorkerKey, WorkerPayload, WorkerScript, WorkerStart};
use crate::mailbox::Sender;
use crate::resource::SourceRequest;
use crate::script::ScriptError;
use crate::view::{EngineEvent, EventRequester, ToPainter, ViewId};

/// Called on `bobcat:runtime`, in a view's realm, with one worker's news.
const WORKER_EVENT_EXPORT: &str = "__BobcatDeliverWorkerEvent";

/// Which workers one view's realm may still name.
///
/// Shared with the host functions that maintain it, so it is `Rc` like
/// [`EventState`](super::EventState): the native `createWorker` export and the
/// command loop that delivers a worker's news are different stack frames on
/// the same thread.
///
/// The keys are the boundary's whole validation, and the only reason this type
/// exists. `bobcat-internal:host` is importable by anything in the realm, so a
/// card could call `postWorkerMessage` with a number it invented; a set of the
/// keys *this view* was issued and has not ended turns that into a JavaScript
/// exception rather than a message delivered to another view's worker. It has
/// to be checked here because the answer has to be synchronous — the thread
/// that knows everything else about a worker is not this one.
pub(super) struct WorkerState<R: EventRequester> {
    view: ViewId,
    /// The group's worker thread. A clone of the one sender the group holds;
    /// there is nothing else to hold.
    workers: Sender<WorkerCommand>,
    notify: ToPainterSender<R>,
    /// The next key no worker of this group has held. A plain counter on this
    /// thread, because this thread is the only one that names a worker.
    next_key: Cell<u64>,
    live: RefCell<FxHashSet<WorkerKey>>,
}

impl<R: EventRequester> WorkerState<R> {
    pub(super) fn new(
        view: ViewId,
        workers: Sender<WorkerCommand>,
        notify: ToPainterSender<R>,
    ) -> Self {
        Self {
            view,
            workers,
            notify,
            next_key: Cell::new(1),
            live: RefCell::default(),
        }
    }

    /// Names one worker, tells the group to make room for it, and tells this
    /// view's painter to fetch its script — in that order, which is the whole
    /// of the ordering anything downstream depends on.
    ///
    /// Two sends and no waiting, which is what HTML's constructor is: the
    /// script is fetched, the realm built and the queue flushed on the other
    /// side of this call, and the card holds a `Worker` before any of it. The
    /// painter's answer goes straight to the worker thread and cannot arrive
    /// before the `Start` does, because it cannot exist until the painter has
    /// seen the notification sent below.
    fn create(&self, url: &str, name: &str) -> WorkerKey {
        let key = WorkerKey::new(self.next_key.get());
        self.next_key.set(key.get() + 1);
        self.live.borrow_mut().insert(key);
        let _ = self.workers.send((
            None,
            WorkerCommand::Start(WorkerStart {
                key,
                view: self.view,
                name: name.to_owned(),
            }),
        ));
        self.notify.send(ToPainter::RequestSource {
            request: SourceRequest::WorkerScript(url.to_owned()),
            worker: Some(key),
        });
        key
    }

    /// A worker the realm may still name, or the reason it may not.
    fn validate(&self, function: &str, key: WorkerKey) -> Result<(), String> {
        if self.live.borrow().contains(&key) {
            Ok(())
        } else {
            Err(format!("{function} names no live worker of this view"))
        }
    }

    /// Hands one answered script to the thread that will run it.
    fn deliver_script(&self, key: WorkerKey, script: WorkerScript) {
        let _ = self.workers.send((
            None,
            WorkerCommand::Script {
                key,
                script: Ok(script),
            },
        ));
    }

    /// Forgets one worker: it ended, however it ended.
    ///
    /// A worker whose script never arrived is forgotten here too, and the
    /// group's thread is told so the slot it made for the key goes with it.
    fn forget(&self, key: WorkerKey) {
        let _ = self.workers.send((None, WorkerCommand::Terminate { key }));
        self.live.borrow_mut().remove(&key);
    }

    /// Ends every worker this view created, because the view is gone.
    pub(super) fn release(&self) {
        if !std::mem::take(&mut *self.live.borrow_mut()).is_empty() {
            let _ = self
                .workers
                .send((None, WorkerCommand::ReleaseView(self.view)));
        }
    }
}

impl<R: EventRequester> MainThreadRuntime<R> {
    /// The painter answered one worker's script request.
    ///
    /// A script goes straight on to the thread that will run it — this one
    /// has nothing to do with the bytes. A failure is that worker's end, and
    /// takes the path a script that threw on load already takes.
    pub(crate) fn worker_script_loaded(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        key: WorkerKey,
        script: Result<WorkerScript, ScriptError>,
    ) -> Result<(), MainThreadError> {
        match script {
            Ok(script) => {
                self.workers.deliver_script(key, script);
                Ok(())
            }
            Err(error) => self.deliver_worker_event(js_runtime, key, WorkerPayload::Failed(error)),
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
}

impl<R: EventRequester> MainThreadRuntime<R> {
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
            // Whether the realm is up yet is the other thread's business:
            // HTML queues what is posted before a worker's global scope
            // exists, and that queue lives with the scope.
            let _ = state.workers.send((
                None,
                WorkerCommand::Message {
                    key,
                    data: data.to_owned(),
                },
            ));
            Ok(HostValue::Undefined)
        },
    )?;

    let state = Rc::clone(workers);
    install(engine, js_runtime, "terminateWorker", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.terminateWorker";
        let key = worker_key_argument(NAME, arguments, 0)?;
        state.validate(NAME, key)?;
        // Cooperative: the worker thread drops the realm between tasks,
        // because nothing interrupts one mid-call. A worker still fetching
        // its script has no realm yet, and the answer finds no slot.
        state.forget(key);
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
    let HostValue::Number(value) = arguments.get(index).unwrap_or(&HostValue::Undefined) else {
        return Err(format!("{function} expects a number for argument {index}"));
    };
    WorkerKey::from_number(*value)
        .ok_or_else(|| format!("{function} expects a worker key for argument {index}"))
}
