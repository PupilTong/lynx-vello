//! The MTS realm's Worker handles and their message channels.
//!
//! JavaScript keeps weak references to Worker objects and releases a handle
//! through `terminateWorker` when the object is collected or explicitly
//! terminated. Each worker has its own cancellation scope. Releasing a view
//! does not cancel those scopes: MTS first completes its JS disposal protocol.
//! When the realm is released, its remaining senders close naturally.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::quickjs::{ScriptEngine, ScriptRuntime};
use crate::background::{WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerStart};
use crate::esm::{BTS_ENTRY_PREAMBLE, BTS_MODULE_SPECIFIER, HOST_MODULE_SPECIFIER};
use crate::link::{ViewNotice, ViewOutbox};
use crate::resource::{LoadedSource, SourceCompletion, SourceRequest};
use crate::script::ScriptError;

pub(super) const MODULE: &str = "bobcat-internal";
pub(super) const SOURCE: &str = crate::esm::runtime_source!("worker");

/// Issued on bobcat-main, once per group. No cross-thread allocator or lock.
#[derive(Clone)]
pub(crate) struct WorkerFactory {
    commands: mpsc::UnboundedSender<WorkerCommand>,
    next: Rc<Cell<u64>>,
}

impl WorkerFactory {
    pub(crate) fn new(commands: mpsc::UnboundedSender<WorkerCommand>) -> Self {
        Self {
            commands,
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
        base_url: &str,
        background_entry: Option<String>,
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
        });
        let creator = Rc::downgrade(&owner);
        let base_url = base_url.to_owned();
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "createWorker",
            2,
            Box::new(move |arguments| {
                let creator = creator.upgrade().ok_or("the creating realm has been released")?;
                let specifier = string(arguments, 0)?.to_owned();
                let name = string(arguments, 1)?.to_owned();
                let id = creator.factory.next.get();
                creator
                    .factory
                    .next
                    .set(id.checked_add(1).ok_or("worker ids exhausted")?);
                let key = WorkerKey::new(id);
                let script = creator.start(key, name)?;
                if specifier == BTS_MODULE_SPECIFIER {
                    let mut source = BTS_ENTRY_PREAMBLE.to_owned();
                    source.push_str("import { __BobcatStartBTS } from \"bobcat:bts-runtime\";\n__BobcatStartBTS(async () => {\n");
                    if let Some(entry) = &background_entry {
                        let entry =
                            serde_json::to_string(entry).expect("a string is JSON serializable");
                        source.push_str("\nawait import(");
                        source.push_str(&entry);
                        source.push_str(");\n");
                    }
                    source.push_str("});\n");
                    // The built-in background script is this thread's own, so
                    // it answers its own request rather than asking a host
                    // that has no bytes for it.
                    script.complete(Ok(LoadedSource::Entry {
                        source,
                        url: BTS_MODULE_SPECIFIER.to_owned(),
                    }));
                    return Ok(HostValue::String(id.to_string()));
                }
                // The answer travels to the worker task without another turn
                // here: what the host is handed is the far end of the
                // one-shot that already rode to `bobcat-workers` with the
                // `Start` above.
                creator.outbox.notify(ViewNotice::RequestSource {
                    request: SourceRequest::Worker {
                        specifier,
                        base_url: base_url.clone(),
                    },
                    completion: script,
                });
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
                let data = string(arguments, 1)?.to_owned();
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
}

impl WorkerOwner {
    /// Names one worker on `bobcat-workers` and hands back the right to
    /// answer its script.
    ///
    /// Source requests share this worker's own token. Neither a source
    /// completion nor a Worker inherits the view's cancellation token.
    fn start(&self, key: WorkerKey, name: String) -> Result<SourceCompletion, String> {
        let (messages, incoming) = mpsc::unbounded_channel();
        let token = CancellationToken::new();
        let (script, awaiting) = SourceCompletion::new(token.clone());
        let sources = self.outbox.host_outbox(token.clone());
        self.outbox.notify(ViewNotice::WorkerCreated {
            key,
            messages: messages.downgrade(),
        });
        self.factory
            .commands
            .send(WorkerCommand::Start(WorkerStart {
                key,
                name,
                script: awaiting,
                messages: incoming,
                events: self.events.clone(),
                token,
                sources,
            }))
            .map_err(|_| "the worker thread has ended".to_owned())?;
        self.live.borrow_mut().insert(key, messages);
        Ok(script)
    }

    fn post(&self, key: WorkerKey, message: WorkerMessage) {
        if let Some(messages) = self.live.borrow().get(&key) {
            let _ = messages.send(message);
        }
    }

    /// `Worker.terminate()`: the worker takes nothing more, including what is
    /// already queued for it, which is why this is a message rather than
    /// simply dropping the sender.
    fn terminate(&self, key: WorkerKey) {
        if let Some(messages) = self.live.borrow_mut().remove(&key) {
            let _ = messages.send(WorkerMessage::Terminate);
        }
    }

    /// Tells the embedder that one of this realm's workers threw or could not
    /// be started.
    ///
    /// Reported from here because this is the realm's side of its workers: a
    /// `WorkerFailed` is the only lifecycle event a worker produces, and the
    /// outbox it goes out on is the one this side already holds for asking the
    /// host to fetch a worker's script.
    pub(super) fn report_failure(&self, error: ScriptError) {
        self.outbox
            .engine_event(crate::EngineEvent::WorkerFailed(error));
    }

    /// Drops what this side kept of a worker that ended on its own — it
    /// called `close()`, or its script or realm failed — so `live` goes on
    /// naming only the workers still running.
    ///
    /// The sender goes with the entry, which closes that channel. Harmless
    /// either way: there is nothing left listening on it.
    pub(super) fn forget(&self, key: WorkerKey) {
        self.live.borrow_mut().remove(&key);
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
