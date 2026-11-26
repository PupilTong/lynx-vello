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
//! holds one sender on it and speaks three messages — start a context, post
//! to one, stop one — and hears events back. A thread that will not start is
//! a failure to build the *group*, named there, rather than a failure of
//! whichever worker happened to be first — which is what lets everything
//! below it be a plain channel send. The one state `bobcat-main`
//! consults is whether this thread has trapped: a trap reports `Failed` to
//! the creator of every worker still on it and sets a flag, and a `Worker`
//! constructed after that fails at once rather than being sent here.
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
//!   host        ◀─── RequestSource ────── the worker's `HostOutbox`
//!               ──── the answer ────────▶ the worker's own task
//! ```
//!
//! Each worker owns its cancellation token. The MTS Worker object's explicit
//! termination or JS finalizer sends `Terminate`; releasing its realm closes
//! the sender. A view's cancellation does not race ahead of JS app cleanup.
//!
//! A worker's script deliberately skips `bobcat-main`, the request and the
//! answer alike: the thread that owns a view's
//! [`ResourceFetcher`](crate::resource::ResourceFetcher) is its painter, and
//! routing the script through the main thread would queue a worker's script
//! behind whatever synchronous JavaScript that thread is in the middle of.
//! `new Worker(...)` asks the host for nothing. The worker's realm loads the
//! module at its URL as its root module when it boots, and that load is a
//! module request like any import's: the worker sends it from this thread
//! through its own `HostOutbox`, and a task of the worker awaits the answer
//! and completes the module. A worker whose URL is an engine name raises no
//! request, the BTS's `bobcat:bts` among them: the realm's own loader loads
//! the name, or refuses it.
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

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use quickjs_rust_bridge::HostValue;
use terminus::link::clock::SpawnedClock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use crate::clock::ClockInstant;
use crate::script::ScriptError;
use crate::threads::ThreadJoin;
use crate::view::EngineError;

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

pub(crate) struct WorkerStart {
    pub(crate) key: WorkerKey,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) messages: mpsc::UnboundedReceiver<WorkerMessage>,
    pub(crate) events: mpsc::UnboundedSender<WorkerEvent>,
    pub(crate) token: CancellationToken,
    pub(crate) sources: crate::link::HostOutbox,
}

pub(crate) enum WorkerCommand {
    Start(WorkerStart),
    #[cfg(test)]
    Panic,
}

pub(crate) enum WorkerMessage {
    Vsync {
        milliseconds: f64,
        enqueued: Option<ClockInstant>,
    },
    Post {
        data: HostValue,
        enqueued: Option<ClockInstant>,
    },
    ModuleCallback {
        call: u64,
        index: u32,
        arguments: Option<String>,
        enqueued: Option<ClockInstant>,
    },
    PerformanceReady {
        enqueued: Option<ClockInstant>,
    },
    MemorySample {
        enqueued: Option<ClockInstant>,
    },
    Terminate,
}

impl WorkerMessage {
    #[cfg(test)]
    pub(crate) const fn post(data: HostValue) -> Self {
        Self::Post {
            data,
            enqueued: None,
        }
    }
}

pub(crate) struct WorkerEvent {
    pub(crate) key: WorkerKey,
    pub(crate) payload: WorkerPayload,
}

pub(crate) enum WorkerPayload {
    Message(HostValue),
    Errored(ScriptError),
    Failed(ScriptError),
    Closed,
}

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
            *sink.borrow_mut() = arguments.first().map(|argument| argument.to_value());
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

pub(crate) struct WorkerHome {
    commands: mpsc::UnboundedSender<WorkerCommand>,
    trapped: Arc<AtomicBool>,
    #[expect(dead_code, reason = "held to wait for the worker thread on drop")]
    thread: ThreadJoin,
}

impl WorkerHome {
    pub(crate) fn start(clock: SpawnedClock) -> Result<Self, EngineError> {
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let trapped = Arc::new(AtomicBool::new(false));
        let thread = ThreadBuilder::new()
            .name("bobcat-workers".to_owned())
            .spawn({
                let trapped = Arc::clone(&trapped);
                move || {
                    clock.align_to_spawner();
                    thread::run(command_receiver, &trapped);
                }
            })
            .map_err(|error| EngineError::Thread {
                name: "worker",
                message: error.to_string(),
            })?;
        Ok(Self {
            commands,
            trapped,
            thread: ThreadJoin::new(thread),
        })
    }

    pub(crate) fn commands(&self) -> mpsc::UnboundedSender<WorkerCommand> {
        self.commands.clone()
    }

    pub(crate) fn trapped(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.trapped)
    }
}
