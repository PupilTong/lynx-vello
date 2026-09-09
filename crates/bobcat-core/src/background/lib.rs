//! The group's worker realms: a second thread, and the second `QuickJS`
//! runtime every `Worker` in the group shares.
//!
//! A worker runtime is separate from `bobcat-main`'s for the reason a worker
//! exists at all — script that must not stop the thread that owns the
//! document. `QuickJS` binds a runtime to one thread, so putting the workers'
//! runtime on a thread of its own is also what makes "a worker cannot touch
//! the document" a fact about the program rather than a rule someone has to
//! keep: there is no path from a worker realm to a `LynxDocument`, and no
//! value of either runtime can be named by the other.
//!
//! One thread per group, not per worker, and the group owns it — not
//! `bobcat-main`. Every `Worker` any view in the group creates gets a realm on
//! this one runtime, the way every view gets a realm on `bobcat-main`'s.
//! Realms on one runtime share a heap, an atom table and a job queue, so a
//! second worker costs a global object and a module graph rather than a heap —
//! at the price of the group's workers taking turns.
//!
//! The thread is started with the group and joined with it, beside
//! `bobcat-main`. A thread that will not start is a failure to build the
//! *group*, named there, rather than a failure of whichever worker happened
//! to be first — which is what lets everything below it be a plain channel
//! send with no state to consult.
//!
//! # Who talks to whom
//!
//! Everything about a worker's *life* is here: which ones exist, which are
//! still waiting for a script, what was posted to one before its realm was up.
//! `bobcat-main` holds no worker state at all — it names one and forwards, and
//! hears back what the worker had to say.
//!
//! ```text
//!   bobcat-main ──── Start / Message / Terminate ────▶ bobcat-workers
//!               ◀──────── ToMain::Worker ─────────────
//!
//!   painter     ──────── WorkerCommand::Script ──────▶
//! ```
//!
//! A worker's *answer* deliberately skips `bobcat-main`: the thread that owns
//! a view's [`ResourceFetcher`](crate::resource::ResourceFetcher) is its
//! painter, and routing the script through the main thread would queue a
//! worker's boot behind whatever synchronous JavaScript that thread is in the
//! middle of. The *ask* rides the link the realm already has, because `new
//! Worker(...)` runs on `bobcat-main` anyway: it sends `Start` here and one
//! `FetchWorkerScript` to its painter, in that order, and the two land on one
//! channel in that order — the painter's answer cannot exist until it has
//! seen a notification `bobcat-main` sent after the `Start`.
//!
//! What a worker says goes back on the group's own mailbox, addressed to the
//! view whose realm created it — the same FIFO and the same addressing every
//! other thing `bobcat-main` serves uses. One park, no selector, and a view
//! that has been released drops its workers' news for free.
//!
//! # What is shared, and where it lives
//!
//! A worker realm is a realm, so it is built out of the same machinery a
//! view's realm is: the `QuickJS` wrapper, the timer schedule, `EventTarget`.
//! That machinery is shared and lives with the view realms under
//! [`main::runtime`](crate::main::runtime); only what a *worker* realm is, and
//! how the group manages one, is here.

mod scope;
#[cfg(test)]
mod tests;
mod thread;

#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use crate::mailbox::{Mailbox, Sender};
use crate::script::ScriptError;
use crate::threads::{self, JoinHandle};
use crate::view::{EngineError, ToMain, ViewId};

/// Names one `Worker` for the life of its group.
///
/// Issued by the realm that constructs one and never reused, so a message
/// still in flight for a worker that has ended cannot find a later one
/// wearing its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct WorkerKey(u64);

impl WorkerKey {
    pub(crate) const fn new(id: u64) -> Self {
        Self(id)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// One worker to start: everything the group knows about it before its script
/// is in hand.
///
/// No URL: the thread that fetches is the one that answers, and its answer
/// carries the resolved URL the module is named by.
pub(crate) struct WorkerStart {
    pub(crate) key: WorkerKey,
    /// The view whose realm created it, and whose realm its messages reach.
    pub(crate) view: ViewId,
    /// The worker's `self.name`, empty when the constructor named none.
    pub(crate) name: String,
}

/// One worker script, resolved, fetched and decoded by the thread that owns
/// the fetcher.
#[derive(Debug)]
pub(crate) struct WorkerScript {
    pub(crate) source: String,
    /// The resolved URL, which is what the worker's module is named by and
    /// what every error against it reports.
    pub(crate) url: String,
}

/// An entry module and the sources its realm can import. Sources belong to
/// this worker, even when another worker uses the same URLs.
pub(crate) struct WorkerProgram {
    pub(crate) entry: WorkerScript,
    pub(crate) modules: Vec<WorkerScript>,
}

impl From<WorkerScript> for WorkerProgram {
    fn from(entry: WorkerScript) -> Self {
        Self {
            entry,
            modules: Vec::new(),
        }
    }
}

impl WorkerProgram {
    pub(crate) fn importing(mut bootstrap: WorkerScript, entry: WorkerScript) -> Self {
        let entry_url = serde_json::to_string(&entry.url).expect("a string is JSON serializable");
        bootstrap.source.push_str("\nawait import(");
        bootstrap.source.push_str(&entry_url);
        bootstrap.source.push_str(");\n");
        Self {
            entry: bootstrap,
            modules: vec![entry],
        }
    }
}

/// Everything the worker thread is ever told.
pub(crate) enum WorkerCommand {
    /// A realm constructed a `Worker`. Nothing about it is running yet: this
    /// is what makes the key live and what gives the queue below something to
    /// queue in front of.
    Start(WorkerStart),
    /// A painter answered one `FetchWorkerScript`, with the script or the
    /// reason there is none.
    Script {
        key: WorkerKey,
        script: Result<WorkerProgram, String>,
    },
    /// One JSON-encoded message for a worker's realm.
    Message { key: WorkerKey, data: String },
    /// `Worker.terminate()`: end it between tasks and drop its realm.
    Terminate { key: WorkerKey },
    /// A view is gone, and with it every worker it created.
    ReleaseView(ViewId),
}

/// The worker thread → `bobcat-main`, on the group's own mailbox, addressed
/// to the view whose realm created the worker.
pub(crate) struct WorkerEvent {
    pub(crate) view: ViewId,
    pub(crate) key: WorkerKey,
    pub(crate) payload: WorkerPayload,
}

pub(crate) enum WorkerPayload {
    /// A JSON-encoded `postMessage` from the worker.
    Message(String),
    /// Something in the worker threw and it is still running — a timer
    /// callback, which HTML reports at the worker and then at its parent
    /// without ending either.
    Errored(ScriptError),
    /// The worker's script could not be fetched, or its realm could not be
    /// built. The realm is gone with it; nothing more will ever arrive under
    /// this key.
    Failed(ScriptError),
    /// The worker ended itself with `close()`.
    Closed,
}

/// The group's right to talk to `bobcat-workers`, and to end it.
///
/// One per [`LynxGroup`](crate::LynxGroup), started beside `bobcat-main` and
/// joined after it. Everything that names a worker — the main thread, each
/// painter — holds a [`Sender`](flume::Sender) cloned from here and nothing
/// else: there is no shared state to guard, so there is no lock, no atomic
/// and no handle to pass around.
pub(crate) struct WorkerHome {
    /// `None` once the goodbye has been said, which is what dropping the last
    /// sender is.
    commands: Option<Sender<WorkerCommand>>,
    thread: Option<JoinHandle>,
}

impl WorkerHome {
    /// Starts the group's worker thread.
    ///
    /// Eager, beside `bobcat-main`: a group pays one parked thread and one
    /// `QuickJS` runtime whether or not a card ever constructs a `Worker`,
    /// and in exchange every path below is one send. Starting it on the first
    /// worker instead would buy that back with a lock, a state machine and a
    /// second way for a worker to fail.
    ///
    /// # Errors
    ///
    /// [`EngineError::Thread`] if the thread will not start, which is a
    /// failure to build the group.
    pub(crate) fn start(to_main: Sender<ToMain>) -> Result<Self, EngineError> {
        let (commands, command_receiver) = Mailbox::channel();
        let thread = ThreadBuilder::new()
            .name("bobcat-workers".to_owned())
            .spawn(move || thread::run(&command_receiver, &to_main))
            .map_err(|error| EngineError::Thread {
                name: "worker",
                message: error.to_string(),
            })?;
        Ok(Self {
            commands: Some(commands),
            thread: Some(thread),
        })
    }

    /// The sending end, for anything that names a worker.
    pub(crate) fn commands(&self) -> Sender<WorkerCommand> {
        self.commands
            .clone()
            .expect("a group hands out senders until it is dropped")
    }

    /// Ends the thread and waits for it.
    ///
    /// The goodbye *is* dropping the last sender, so it is taken here rather
    /// than left to the field's own drop: a wait reachable with the sender
    /// still alive would never return.
    pub(crate) fn join(&mut self) {
        drop(self.commands.take());
        if let Some(thread) = self.thread.take() {
            threads::join(thread);
        }
    }
}
