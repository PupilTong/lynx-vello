//! The MTS realm's Worker handles and their message channels.
//!
//! JavaScript keeps weak references to Worker objects and releases a handle
//! through `terminateWorker` when the object is collected or explicitly
//! terminated. Each worker has its own cancellation scope. Releasing a view
//! does not cancel those scopes: MTS first completes its JS disposal protocol.
//! When the realm is released, its remaining senders close naturally.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::quickjs::{ScriptEngine, ScriptRuntime};
use crate::background::{
    WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart,
};
use crate::esm::{BTS_MODULE_SPECIFIER, ENGINE_MODULE_PREFIXES, HOST_MODULE_SPECIFIER};
use crate::link::{ViewNotice, ViewOutbox};
use crate::resource::{SourceCompletion, SourceRequest};
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

    /// Installs the three members a realm creates and drives workers through,
    /// and hands back the owner they share and the channel everything they say
    /// arrives on.
    pub(super) fn install(
        &self,
        engine: &mut ScriptEngine,
        runtime: &mut ScriptRuntime,
        outbox: ViewOutbox,
    ) -> Result<(Rc<WorkerOwner>, mpsc::UnboundedReceiver<WorkerEvent>), ScriptError> {
        let (events, incoming) = mpsc::unbounded_channel();
        // The MTS runtime owns the channels. Host functions borrow that owner
        // weakly: cleanup jobs queued while a realm is released must not keep
        // the owner, its channels, or the group thread alive.
        let owner = Rc::new(WorkerOwner {
            factory: self.clone(),
            outbox,
            events,
            live: RefCell::default(),
            sources: RefCell::default(),
        });
        let creator = Rc::downgrade(&owner);
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "createWorker",
            3,
            Box::new(move |arguments| {
                let creator = creator
                    .upgrade()
                    .ok_or("the creating realm has been released")?;
                let specifier = string(arguments, 0)?;
                let name = string(arguments, 1)?.to_owned();
                // Where the specifier resolves from: the MTS entry's response
                // URL, which the realm holds as `__Card__` and hands over with
                // every construction. It is absolute: an entry answered from
                // one that is not fails the view's startup before it runs.
                // Rust joins the specifier to it by URL rules and still does
                // not remember it. An absolute URL joins to itself,
                // `bobcat:bts` included. A specifier that does not resolve
                // starts nothing: no id, no `Start`, no request, and `null`
                // back, which the realm throws as a `SyntaxError`.
                let base_url = string(arguments, 2)?;
                let Ok(url) = url::Url::parse(base_url).and_then(|base| base.join(specifier))
                else {
                    return Ok(HostValue::Null);
                };
                let url = String::from(url);
                let id = creator.factory.next.get();
                creator
                    .factory
                    .next
                    .set(id.checked_add(1).ok_or("worker ids exhausted")?);
                let key = WorkerKey::new(id);
                // Each worker's own end signal, which its script request is
                // cancelled with as well.
                let token = CancellationToken::new();
                // A worker's script is asked for here, on the thread whose
                // realm constructed it, unless its URL is an engine name: the
                // host is never asked for one of those, and the realm's own
                // loader loads it or refuses it.
                let (script, completion) = if ENGINE_MODULE_PREFIXES
                    .iter()
                    .any(|prefix| url.starts_with(prefix))
                {
                    (None, None)
                } else {
                    let (completion, script) = SourceCompletion::new(token.clone());
                    (Some(script), Some(completion))
                };
                // The worker whose URL is `bobcat:bts` is the view's
                // background thread, and its diagnostics are the BTS's. That
                // is the one thing its `Start` says differently: the view's
                // data reaches it in the `initialize` message the realm posts
                // to it.
                let source = if url == BTS_MODULE_SPECIFIER {
                    ScriptSource::Background
                } else {
                    ScriptSource::Worker(WorkerId::from(key))
                };
                let (messages, incoming) = mpsc::unbounded_channel();
                let start = WorkerStart {
                    key,
                    name,
                    url,
                    script,
                    source,
                    messages: incoming,
                    events: creator.events.clone(),
                    sources: creator.outbox.host_outbox(token.clone()),
                    token,
                };
                creator.start(start, messages, completion);
                Ok(HostValue::String(id.to_string()))
            }),
        )?;
        let sender = Rc::downgrade(&owner);
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "sendWorkerMessage",
            2,
            Box::new(move |arguments| {
                let key = key(arguments)?;
                // Any value the boundary carries. A value the serializer
                // refuses throws in the realm that called `postMessage`, so
                // nothing here has to decide what a message may be.
                let data = arguments.get(1).cloned().unwrap_or(HostValue::Undefined);
                if let Some(sender) = sender.upgrade() {
                    sender.post(key, WorkerMessage::Post(data));
                }
                Ok(HostValue::Undefined)
            }),
        )?;
        let terminator = Rc::downgrade(&owner);
        engine.register_host_module_function(
            runtime,
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
        Ok((owner, incoming))
    }
}

/// One realm's whole side of its workers.
pub(super) struct WorkerOwner {
    factory: WorkerFactory,
    outbox: ViewOutbox,
    /// Cloned into every `Start`, so the channel stays open while the realm
    /// does even when it has no worker at all.
    events: mpsc::UnboundedSender<WorkerEvent>,
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
    /// Sends `start` to `bobcat-workers`, keeps `messages`, the sending end of
    /// its message channel, while the worker runs, and asks the host for the
    /// worker's script with `completion`, which is `None` for a URL the host
    /// is never asked for.
    ///
    /// The worker's source is recorded before anything is sent, so a worker
    /// that fails at once has one too. Source requests share this worker's
    /// own token, which `start` carries. Neither a source completion nor a
    /// Worker inherits the view's cancellation token.
    ///
    /// A worker thread that has trapped, or one whose inbox is closed, fails
    /// the worker at once, as one whose script could not be fetched does. A
    /// worker that has already failed is still a worker to the script that
    /// named it: its `Failed` is queued on this realm's own channel and
    /// reaches the script as an `error` event. Nothing is sent to the thread,
    /// kept in `live` or asked of the host, and the answer `start` carried is
    /// dropped with it.
    fn start(
        &self,
        start: WorkerStart,
        messages: mpsc::UnboundedSender<WorkerMessage>,
        completion: Option<SourceCompletion>,
    ) {
        let key = start.key;
        self.sources.borrow_mut().insert(key, start.source);
        if !self.factory.trapped.load(Ordering::Acquire) {
            self.outbox.notify(ViewNotice::WorkerCreated {
                key,
                messages: messages.downgrade(),
            });
            let url = start.url.clone();
            let started = self.factory.commands.send(WorkerCommand::Start(start));
            // On a refused send `messages` drops at the end of this block, so
            // the handle `WorkerCreated` registered no longer upgrades and the
            // view sweeps it.
            if started.is_ok() {
                self.live.borrow_mut().insert(key, messages);
                // The answer travels to the worker task without another turn
                // here: what the host is handed is the far end of the
                // one-shot that already rode to `bobcat-workers` with the
                // `Start` above.
                if let Some(completion) = completion {
                    self.outbox.notify(ViewNotice::RequestSource {
                        request: SourceRequest::Module(url),
                        completion,
                    });
                }
                return;
            }
        }
        let _ = self.events.send(WorkerEvent {
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
    /// Its source goes too: the script has let go of the key, so whatever the
    /// worker says after this is dropped before it reaches a `Worker` object,
    /// and a `terminate()` is reported to no one.
    fn terminate(&self, key: WorkerKey) {
        self.sources.borrow_mut().remove(&key);
        if let Some(messages) = self.live.borrow_mut().remove(&key) {
            let _ = messages.send(WorkerMessage::Terminate);
        }
    }

    /// Tells the embedder that the worker `source` names threw, or, when
    /// `ended`, that it ended without being told to.
    ///
    /// Reported from here because this is the realm's side of its workers:
    /// `WorkerThrew` and `WorkerEnded` are the only lifecycle events a worker
    /// produces, and the outbox they go out on is the one this side already
    /// holds for asking the host to fetch a worker's script.
    pub(super) fn report_failure(&self, source: ScriptSource, ended: bool, error: ScriptError) {
        self.outbox.engine_event(if ended {
            crate::EngineEvent::WorkerEnded { source, error }
        } else {
            crate::EngineEvent::WorkerThrew { source, error }
        });
    }

    /// Drops what this side kept of a worker that ended on its own — it
    /// called `close()`, or its script or realm failed — so `live` goes on
    /// naming only the workers still running.
    ///
    /// The sender goes with the entry, which closes that channel. Harmless
    /// either way: there is nothing left listening on it.
    pub(super) fn forget(&self, key: WorkerKey) {
        self.live.borrow_mut().remove(&key);
        self.sources.borrow_mut().remove(&key);
    }

    /// Which realm the worker under `key` is, while the script still holds
    /// that key.
    pub(super) fn source_of(&self, key: WorkerKey) -> Option<ScriptSource> {
        self.sources.borrow().get(&key).copied()
    }

    /// How many workers this realm still has running.
    #[cfg(test)]
    pub(super) fn live_workers(&self) -> usize {
        self.live.borrow().len()
    }
}

fn string(arguments: &[HostValue], index: usize) -> Result<&str, String> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value),
        _ => Err(format!(
            "Worker host operation expects string argument {index}"
        )),
    }
}

fn key(arguments: &[HostValue]) -> Result<WorkerKey, String> {
    string(arguments, 0)?
        .parse()
        .map(WorkerKey::new)
        .map_err(|_| "invalid worker key".to_owned())
}
