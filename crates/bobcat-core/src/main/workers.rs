//! The main realm's Worker handle: start a worker realm, ask the host for its
//! script, and route what the realm posts to it.
//!
//! Nothing about a worker's *state* is here. This side owns exactly one thing
//! per worker — the sending end of its message channel — and a released realm
//! sends a `Terminate` on every one of them, which is how a view stops the
//! workers it created. The channel closing behind that message ends a worker
//! too, but it is the backstop rather than the protocol.
//!
//! What travels with a worker besides that channel is a cancellation token,
//! minted here as a child of the view's own. It is the *signal* a worker's
//! tasks wake on, where the `Terminate` is the message: a view released
//! before its realm could say anything cancels its token on the embedder's
//! thread, and every worker it created ends without this side taking a turn.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};

use super::quickjs::{ScriptEngine, ScriptRuntime};
use crate::background::{WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerStart};
use crate::esm::{BTS_ENTRY_PREAMBLE, BTS_MODULE_SPECIFIER, HOST_MODULE_SPECIFIER};
use crate::link::{ViewNotice, ViewOutbox};
use crate::resource::{LoadedSource, SourceRequest};
use crate::script::ScriptError;

pub(super) const MODULE: &str = "bobcat-internal";
pub(super) const SOURCE: &str = include_str!("../../../../packages/bobcat-element/src/worker.mjs");

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
        // The native functions hold clones of the owner until the realm is
        // dropped, including when entry boot fails after it constructed
        // workers. The caller keeps one too, which is what outlives them.
        let owner = Rc::new(WorkerOwner {
            factory: self.clone(),
            outbox,
            events,
            live: RefCell::default(),
        });
        let creator = Rc::clone(&owner);
        let base_url = base_url.to_owned();
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "createWorker",
            2,
            Box::new(move |arguments| {
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
                    if let Some(entry) = &background_entry {
                        let entry =
                            serde_json::to_string(entry).expect("a string is JSON serializable");
                        source.push_str("\nawait import(");
                        source.push_str(&entry);
                        source.push_str(");\n");
                    }
                    // The built-in background script is this thread's own, so
                    // it answers its own request rather than asking a host
                    // that has no bytes for it.
                    let _ = script.send(Ok(LoadedSource::Entry {
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
                    completion: creator.outbox.completion_for(script),
                });
                Ok(HostValue::String(id.to_string()))
            }),
        )?;
        let sender = Rc::clone(&owner);
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "sendWorkerMessage",
            2,
            Box::new(move |arguments| {
                let key = key(arguments)?;
                let data = string(arguments, 1)?.to_owned();
                sender.post(key, WorkerMessage::Post(data));
                Ok(HostValue::Undefined)
            }),
        )?;
        let terminator = Rc::clone(&owner);
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "terminateWorker",
            1,
            Box::new(move |arguments| {
                terminator.terminate(key(arguments)?);
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
    /// The one thing this side keeps per worker: the right to tell it to
    /// stop. What ends a worker is a message from the realm that created it —
    /// `terminate()`, or the `Drop` below as that realm is released. The
    /// channel closing when this map goes is the backstop, not the protocol.
    ///
    /// It holds exactly the workers still running: [`Self::start`] enters one,
    /// and it leaves again the moment this side learns the worker is over —
    /// [`Self::terminate`] for the realm's own `terminate()`, and
    /// [`Self::forget`] for a worker that ended on its own.
    live: RefCell<FxHashMap<WorkerKey, mpsc::UnboundedSender<WorkerMessage>>>,
}

impl Drop for WorkerOwner {
    /// A released realm stops its own workers rather than leaving each to
    /// notice that nobody is talking to it any more.
    ///
    /// JavaScript first, then the message: the realm's host functions hold
    /// clones of this owner and the runtime holds the last one, so this runs
    /// once that realm's context has been freed and every worker it names is
    /// one this view will never hear from again.
    ///
    /// The message is the protocol. Two things behind it are the backstop,
    /// for a worker whose realm was gone before it could speak: the senders
    /// drained here going out of scope with this statement, and the view's
    /// token, whose children every one of these workers holds.
    fn drop(&mut self) {
        for (_, messages) in self.live.borrow_mut().drain() {
            let _ = messages.send(WorkerMessage::Terminate);
        }
    }
}

impl WorkerOwner {
    /// Names one worker on `bobcat-workers` and hands back the right to
    /// answer its script.
    ///
    /// The token that rides with it is a child of this view's, so cancelling
    /// the view's cancels every worker's — including the ones whose `Start`
    /// has not been served yet.
    fn start(
        &self,
        key: WorkerKey,
        name: String,
    ) -> Result<oneshot::Sender<Result<LoadedSource, crate::LynxViewError>>, String> {
        let (script, awaiting) = oneshot::channel();
        let (messages, incoming) = mpsc::unbounded_channel();
        self.factory
            .commands
            .send(WorkerCommand::Start(WorkerStart {
                key,
                name,
                script: awaiting,
                messages: incoming,
                events: self.events.clone(),
                token: self.outbox.token().child_token(),
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

    /// Drops what this side kept of a worker that ended on its own — it
    /// called `close()`, or its script or realm failed — so `live` goes on
    /// naming only the workers still running and the `Drop` above sends no
    /// `Terminate` to a task that has already returned.
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
