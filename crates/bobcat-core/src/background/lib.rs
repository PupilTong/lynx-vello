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
//!   host        ──── the script ────────▶ the worker's own task
//! ```
//!
//! Each worker owns its cancellation token. The MTS Worker object's explicit
//! termination or JS finalizer sends `Terminate`; releasing its realm closes
//! the sender. A view's cancellation does not race ahead of JS app cleanup.
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

use quickjs_rust_bridge::HostValue;
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
    /// The worker's `self.name`, empty when the constructor named none.
    pub(crate) name: String,
    /// Its script, answered by whichever thread owns the creating view's
    /// fetcher. A `Start` for the built-in background context arrives with
    /// this already answered.
    pub(crate) script: oneshot::Receiver<Result<LoadedSource, LynxViewError>>,
    /// What the MTS Worker object posts. Its finalizer or explicit terminate
    /// sends `Terminate`; releasing the MTS realm closes the channel.
    pub(crate) messages: mpsc::UnboundedReceiver<WorkerMessage>,
    /// Where this worker reports, which is the creating view's own channel.
    pub(crate) events: mpsc::UnboundedSender<WorkerEvent>,
    /// This worker's end signal, independent of its creating view's token.
    pub(crate) token: CancellationToken,
    /// Sources and frame demand reach the host directly, under this worker's lifetime.
    pub(crate) sources: crate::link::HostOutbox,
}

/// Everything the worker thread is ever told.
pub(crate) enum WorkerCommand {
    /// A realm constructed a `Worker`.
    Start(WorkerStart),
}

/// Everything one worker in particular is ever told.
pub(crate) enum WorkerMessage {
    /// A display opportunity from the painter, handled even while entry awaits.
    Vsync(f64),
    /// One value, primitive or structured clone, for the worker's realm.
    Post(HostValue),
    /// Explicit termination or GC of the MTS Worker object: end it between
    /// tasks and discard what was queued behind this.
    Terminate,
}

/// One worker realm → the view whose realm created it.
pub(crate) struct WorkerEvent {
    pub(crate) key: WorkerKey,
    pub(crate) payload: WorkerPayload,
}

pub(crate) enum WorkerPayload {
    /// One value, primitive or structured clone, from the worker's
    /// `postMessage`.
    Message(HostValue),
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

/// One wire value, built from the JavaScript expression that produces it.
///
/// A test that plays one side of the transport by hand used to write the
/// message as JSON text. It cannot any more: a structured clone is opaque to
/// the host, produced only by a realm, so the expression is evaluated in a
/// throwaway realm and what crosses the boundary is kept. The stream is
/// self-contained, so a clone written here reads in whichever realm the test
/// hands it to.
#[cfg(test)]
pub(crate) fn wire_value(expression: &str) -> HostValue {
    use std::cell::RefCell;
    use std::rc::Rc;

    use quickjs_rust_bridge::{EvalOptions, EvalSource, Runtime};

    let captured: Rc<RefCell<Option<HostValue>>> = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&captured);
    let runtime = Runtime::new().expect("a throwaway runtime");
    let mut realm = runtime.create_context().expect("a throwaway realm");
    realm
        .define_global_function("__capture", 1, move |arguments| {
            *sink.borrow_mut() = arguments.first().cloned();
            Ok(HostValue::Undefined)
        })
        .expect("installing the capture function");
    realm
        .evaluate(
            EvalSource::new(&format!("__capture({expression})")),
            EvalOptions::default(),
        )
        .expect("the wire value evaluates");
    captured
        .borrow_mut()
        .take()
        .expect("the wire value crossed the boundary")
}

/// One wire value as JSON text, for an assertion to compare against.
///
/// A description, not the transport: the clone is read back into a throwaway
/// realm and stringified there, because a structured clone carries more than
/// JSON can spell and nothing in Rust can look inside it. A test that cares
/// about what JSON cannot spell uses [`wire_matches`] instead.
#[cfg(test)]
pub(crate) fn wire_json(value: &HostValue) -> String {
    use quickjs_rust_bridge::{EvalOptions, EvalSource, Runtime};

    let runtime = Runtime::new().expect("a throwaway runtime");
    let mut realm = runtime.create_context().expect("a throwaway realm");
    let value = value.clone();
    realm
        .define_global_function("__value", 0, move |_| Ok(value.clone()))
        .expect("installing the value function");
    let answer = realm
        .evaluate(
            EvalSource::new("String(JSON.stringify(__value()))"),
            EvalOptions::default(),
        )
        .expect("the wire value stringifies");
    String::from_utf16(&answer.to_utf16().expect("a JavaScript string"))
        .expect("well-formed UTF-16")
}

/// Whether a wire value is the object this JavaScript predicate accepts.
///
/// The mirror of [`wire_value`], for a test that has to recognize a message
/// rather than build one: the clone is read back into a throwaway realm and
/// the predicate is applied there, because nothing in Rust can look inside it.
#[cfg(test)]
pub(crate) fn wire_matches(value: &HostValue, predicate: &str) -> bool {
    use quickjs_rust_bridge::{EvalOptions, EvalSource, Runtime};

    let runtime = Runtime::new().expect("a throwaway runtime");
    let mut realm = runtime.create_context().expect("a throwaway realm");
    let value = value.clone();
    realm
        .define_global_function("__value", 0, move |_| Ok(value.clone()))
        .expect("installing the value function");
    realm
        .evaluate(
            EvalSource::new(&format!("Boolean(({predicate})(__value()))")),
            EvalOptions::default(),
        )
        .is_ok_and(|answer| answer.as_boolean() == Some(true))
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
