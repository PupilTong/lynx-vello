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
//! The thread is started by `LynxGroup::new`, beside `bobcat-main` rather
//! than by it, and joined by the group handle's drop once `bobcat-main` has
//! returned. It is a runtime environment of the group's own: `bobcat-main`
//! holds one sender on it and speaks three messages — start a context with
//! its script, post to one, stop one — and hears events back. A thread that
//! will not start is a failure to build the *group*, named there, rather than
//! a failure of whichever worker happened to be first — which is what lets
//! everything below it be a plain channel send with no state to consult.
//!
//! # Who talks to whom
//!
//! Everything about a worker's *life* is here: which ones exist, and what
//! each is waiting for. `bobcat-main` holds no worker state at all beyond one
//! sender per worker — it names one and forwards, and hears back what the
//! worker had to say.
//!
//! ```text
//!   bobcat-main ──── Start ─────────────▶ bobcat-workers
//!               ──── Post / Terminate ──▶ the worker's own task
//!               ◀─────── WorkerEvent ────
//!
//!   the view's token ─── cancel ────────▶ the worker's own token (its child)
//!
//!   host        ──── the script ────────▶ the worker's own task
//! ```
//!
//! The token is the one thing on that picture that needs no turn anywhere: a
//! worker's is a child of the token its creating view was built with, so a
//! released view ends every worker it created by cancelling one thing on the
//! embedder's own thread. The `Terminate` above stays the protocol — it is
//! what `terminate()` and a released realm say, and it is what discards what
//! was queued behind it — and the token is the signal a worker's tasks wake
//! on, and the backstop for a realm that was gone before it could speak.
//!
//! A worker's *answer* deliberately skips `bobcat-main`: the thread that owns
//! a view's [`ResourceFetcher`](crate::resource::ResourceFetcher) is its
//! painter, and routing the script through the main thread would queue a
//! worker's boot behind whatever synchronous JavaScript that thread is in the
//! middle of. The ask rides the link the realm already has, because `new
//! Worker(...)` runs on `bobcat-main` anyway; what the host is handed is the
//! far end of a one-shot whose receiving end already travelled here inside
//! the `Start`, so no ordering between the two has to be arranged.
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

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use crate::resource::LoadedSource;
use crate::script::ScriptError;
use crate::threads::ThreadJoin;
use crate::view::{EngineError, LynxViewError};

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

/// One worker to start: everything it will ever be given, in one message.
///
/// No URL and no state: the thread that fetches is the one that answers, its
/// answer carries the resolved URL the module is named by, and everything
/// else a worker has — what is posted to it, what it says back — is a channel
/// that arrives with it.
pub(crate) struct WorkerStart {
    pub(crate) key: WorkerKey,
    /// The view's built-in BTS has an app lifetime; an ordinary Worker does not.
    pub(crate) background: bool,
    /// The worker's `self.name`, empty when the constructor named none.
    pub(crate) name: String,
    /// Its script, answered by whichever thread owns the creating view's
    /// fetcher. A `Start` for the built-in background context arrives with
    /// this already answered.
    pub(crate) script: oneshot::Receiver<Result<LoadedSource, LynxViewError>>,
    /// What the creating realm posts, and what tells this worker to stop: a
    /// released realm sends a [`WorkerMessage::Terminate`] on it before it
    /// drops the sending end. The closing itself ends the worker too, for the
    /// realm that was gone before it could say anything.
    pub(crate) messages: mpsc::UnboundedReceiver<WorkerMessage>,
    /// Where this worker reports, which is the creating view's own channel.
    pub(crate) events: mpsc::UnboundedSender<WorkerEvent>,
    /// This worker's end signal: a child of the token its creating view was
    /// built with, minted by the realm that constructed it. So a released view
    /// ends every worker it created without a message reaching each of them
    /// first, and a worker whose realm was gone before it could speak still
    /// wakes.
    pub(crate) token: CancellationToken,
    /// Imported text reaches the view resource host under this worker's cancellation scope.
    pub(crate) sources: crate::link::SourceRequester,
}

/// Everything the worker thread is ever told.
pub(crate) enum WorkerCommand {
    /// A realm constructed a `Worker`.
    Start(WorkerStart),
}

/// Everything one worker in particular is ever told.
pub(crate) enum WorkerMessage {
    /// One JSON-encoded message for the worker's realm.
    Post(String),
    /// `Worker.terminate()`, and a released realm stopping what it created:
    /// end it between tasks and drop its realm, discarding whatever was
    /// queued behind this.
    Terminate,
}

/// One worker realm → the view whose realm created it.
pub(crate) struct WorkerEvent {
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
/// One per [`LynxGroup`](crate::LynxGroup), started by `LynxGroup::new` and
/// joined by the group handle's drop, after `bobcat-main`. Everything that
/// names a worker holds a sender cloned from here and nothing else — one of
/// them is `bobcat-main`'s, which is the whole of what that thread has of this
/// one: there is no shared state to guard, so there is no lock, no atomic and
/// no handle to pass around.
///
/// **The field order is the teardown, and it must stay in this order.** Fields
/// drop in declaration order, so the goodbye — this side's last sender closing
/// the thread's inbox — is said before the join below waits for the thread to
/// answer it. A wait reached with a sender still alive would never return.
pub(crate) struct WorkerHome {
    commands: mpsc::UnboundedSender<WorkerCommand>,
    #[expect(dead_code, reason = "held to wait for the worker thread on drop")]
    thread: ThreadJoin,
}

impl WorkerHome {
    /// Starts the group's worker thread.
    ///
    /// Eager, and beside `bobcat-main` rather than under it: a group pays one
    /// parked thread and one `QuickJS` runtime whether or not a card ever
    /// constructs a `Worker`, and in exchange every path below is one send.
    /// Starting it on the first worker instead would buy that back with a
    /// lock, a state machine and a second way for a worker to fail.
    ///
    /// # Errors
    ///
    /// [`EngineError::Thread`] if the thread will not start, which is a
    /// failure to build the group.
    pub(crate) fn start() -> Result<Self, EngineError> {
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let thread = ThreadBuilder::new()
            .name("bobcat-workers".to_owned())
            .spawn(move || thread::run(command_receiver))
            .map_err(|error| EngineError::Thread {
                name: "worker",
                message: error.to_string(),
            })?;
        Ok(Self {
            commands,
            thread: ThreadJoin::new(thread),
        })
    }

    /// The sending end, for anything that names a worker.
    pub(crate) fn commands(&self) -> mpsc::UnboundedSender<WorkerCommand> {
        self.commands.clone()
    }
}
