//! A realm's futures: the table one is registered in, and the three members
//! the `Future` class is written over.
//!
//! Not the main thread's and not a worker's — *a realm's*, like
//! [`crate::timers`] and [`crate::require`]. Both kinds get all three, and
//! both drain the settle requests their entries produced in the same place:
//! the owner's epilogue.
//!
//! # Why a table rather than a value
//!
//! Only primitives and structured clones cross the host boundary, so nothing
//! a Rust future resolves to can be handed over as itself, and the future
//! itself certainly cannot. What crosses is a number. The host keeps the
//! operation under it, keeps whatever it settled to under it, and the realm
//! holds a `Future` object that is that number and nothing else.
//!
//! # The two ways out, and why they are exclusive
//!
//! `waitFuture` is **one of the two synchronous waits a realm has**, beside
//! `adoptStyleSheet`'s. It parks the *job* the calling member runs in: every
//! task of this engine thread goes on running — the channel reads, the
//! lifecycle signals, the routing that answers this very request — and no
//! other job runs, so no promise job of this realm's and no entry of a
//! sibling realm's interleaves with it. The wait's first arm, biased, is the
//! requesting realm's end signal, which a task is what cancels: for a view's
//! MTS realm that is the embedder's release, written from its own thread;
//! for a Worker it is the in-band `Terminate` the message consumer reads
//! during the wait, which ends the worker and cancels the token its own
//! requests carry.
//!
//! `settleFuture` is the other way, and it is asynchronous: it hands the
//! future to the owner's epilogue, which spawns a task to await it and then
//! enters the realm to deliver what it settled to. That delivery is a job,
//! and a job cannot run inside another job's wait — which is why a `Future`
//! that has been converted to a Promise refuses a later `wait`, and why the
//! realm's half of this is where that refusal is worded.
//!
//! A timed-out wait cancels nothing. The future goes back into the table and
//! the operation behind it keeps running, so a later `wait` or a `.then` on
//! the same `Future` still answers.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;
use tokio_util::sync::CancellationToken;

use crate::clock::ClockInstant;
use crate::esm::{Export, HOST_MODULE_SPECIFIER};
use crate::jobs::JsThreadHandle;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

const MAX_TIMEOUT_MILLISECONDS: f64 = 2_147_483_647.0;

pub(crate) type Outcome = Result<HostValue, String>;

pub(crate) type HostFuture = Pin<Box<dyn Future<Output = Outcome>>>;

pub(crate) struct FutureTable {
    next_id: Cell<u32>,
    pending: RefCell<FxHashMap<u32, HostFuture>>,
    settled: RefCell<FxHashMap<u32, Outcome>>,
    settle_requests: RefCell<Vec<(u32, HostFuture)>>,
}

impl FutureTable {
    pub(crate) fn new() -> Self {
        Self {
            next_id: Cell::new(1),
            pending: RefCell::new(FxHashMap::default()),
            settled: RefCell::new(FxHashMap::default()),
            settle_requests: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn register(&self, future: impl Future<Output = Outcome> + 'static) -> u32 {
        let id = self.allocate_id();
        self.pending.borrow_mut().insert(id, Box::pin(future));
        id
    }

    pub(crate) fn take_settle_requests(&self) -> Vec<(u32, HostFuture)> {
        std::mem::take(&mut *self.settle_requests.borrow_mut())
    }

    fn park(&self, id: u32, outcome: Outcome) {
        self.settled.borrow_mut().insert(id, outcome);
    }

    fn take(&self, id: u32) -> Result<HostValue, String> {
        match self.settled.borrow_mut().remove(&id) {
            Some(outcome) => outcome,
            None => Err(format!("future {id} has not settled")),
        }
    }

    fn allocate_id(&self) -> u32 {
        loop {
            let id = self.next_id.get();
            self.next_id.set(id.checked_add(1).unwrap_or(1));
            if id != 0
                && !self.pending.borrow().contains_key(&id)
                && !self.settled.borrow().contains_key(&id)
            {
                return id;
            }
        }
    }
}

enum Waited {
    Ended,
    TimedOut,
    Settled(Outcome),
}

pub(crate) fn install(
    engine: &mut ScriptEngine,
    table: &Rc<FutureTable>,
    token: CancellationToken,
    thread: JsThreadHandle,
) -> Result<(), ScriptError> {
    let futures = Rc::clone(table);
    install_member(engine, "waitFuture", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.waitFuture";
        let id = future_id_argument(NAME, arguments, 0)?;
        let deadline = deadline_from(number_argument(NAME, arguments, 1)?);
        let Some(mut future) = futures.pending.borrow_mut().remove(&id) else {
            return Err(format!("future {id} is not pending"));
        };
        let waited = thread.wait(async {
            let mut sleep = std::pin::pin!(crate::clock::sleep_until(
                deadline.unwrap_or_else(ClockInstant::now)
            ));
            tokio::select! {
                biased;
                () = token.cancelled() => Waited::Ended,
                () = &mut sleep, if deadline.is_some() => Waited::TimedOut,
                outcome = &mut future => Waited::Settled(outcome),
            }
        });
        match waited {
            Waited::Ended => Err("the realm ended".to_owned()),
            Waited::TimedOut => {
                futures.pending.borrow_mut().insert(id, future);
                Ok(HostValue::Boolean(false))
            }
            Waited::Settled(outcome) => {
                futures.park(id, outcome);
                Ok(HostValue::Boolean(true))
            }
        }
    })?;

    let futures = Rc::clone(table);
    install_member(engine, "takeFuture", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.takeFuture";
        let id = future_id_argument(NAME, arguments, 0)?;
        futures.take(id)
    })?;

    let futures = Rc::clone(table);
    install_member(engine, "settleFuture", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.settleFuture";
        let id = future_id_argument(NAME, arguments, 0)?;
        let Some(future) = futures.pending.borrow_mut().remove(&id) else {
            return Err(format!("future {id} is not pending"));
        };
        futures.settle_requests.borrow_mut().push((id, future));
        Ok(HostValue::Undefined)
    })?;

    #[cfg(test)]
    {
        let futures = Rc::clone(table);
        install_member(engine, "testFuture", 3, move |arguments| {
            const NAME: &str = "bobcat-internal:host.testFuture";
            let delay = Duration::from_secs_f64(
                number_argument(NAME, arguments, 0)?.clamp(0.0, MAX_TIMEOUT_MILLISECONDS) / 1_000.0,
            );
            let value = string_argument(NAME, arguments, 1)?.to_owned();
            let rejects = boolean_argument(NAME, arguments, 2)?;
            let armed = ClockInstant::now();
            let id = futures.register(async move {
                crate::clock::sleep_until(armed + delay).await;
                if rejects {
                    Err(value)
                } else {
                    Ok(HostValue::String(value))
                }
            });
            Ok(HostValue::Number(f64::from(id)))
        })?;
    }
    Ok(())
}

pub(crate) fn deliver(
    engine: &ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    id: u32,
    outcome: Outcome,
) -> Result<(), ScriptError> {
    let (rejected, value) = match outcome {
        Ok(value) => (false, value),
        Err(reason) => (true, HostValue::String(reason)),
    };
    engine
        .call_module_export(
            js_runtime,
            Export::SettleFuture,
            &[
                HostArgument::Number(f64::from(id)),
                HostArgument::Boolean(rejected),
                value.as_argument(),
            ],
        )
        .map(|_| ())
}

fn deadline_from(milliseconds: f64) -> Option<ClockInstant> {
    if !milliseconds.is_finite() {
        return None;
    }
    let clamped = milliseconds.clamp(0.0, MAX_TIMEOUT_MILLISECONDS);
    Some(ClockInstant::now() + Duration::from_secs_f64(clamped / 1_000.0))
}

fn install_member(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostArgument<'_>]) -> Result<HostValue, String> + 'static,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(HOST_MODULE_SPECIFIER, name, arity, callback)
}

fn number_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<f64, String> {
    match arguments.get(index) {
        Some(&HostArgument::Number(value)) => Ok(value),
        _ => Err(format!("{function} expects a number for argument {index}")),
    }
}

#[cfg(test)]
fn string_argument<'a>(
    function: &str,
    arguments: &[HostArgument<'a>],
    index: usize,
) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(&HostArgument::String(value)) => Ok(value),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}

#[cfg(test)]
fn boolean_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<bool, String> {
    match arguments.get(index) {
        Some(&HostArgument::Boolean(value)) => Ok(value),
        _ => Err(format!("{function} expects a boolean for argument {index}")),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the integer and range checks above make the value a representable id"
)]
fn future_id_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<u32, String> {
    let value = number_argument(function, arguments, index)?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
        return Err(format!(
            "{function} expects a future id for argument {index}"
        ));
    }
    Ok(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settles_to(value: &str) -> impl Future<Output = Outcome> + use<> {
        let value = value.to_owned();
        async move { Ok(HostValue::String(value)) }
    }

    #[test]
    fn ids_start_at_one_so_a_realm_can_test_one_for_truth() {
        let table = FutureTable::new();

        assert_eq!(table.register(settles_to("first")), 1);
        assert_eq!(table.register(settles_to("second")), 2);
    }

    #[test]
    fn an_id_a_live_future_holds_is_never_handed_out_again() {
        let table = FutureTable::new();
        let first = table.register(settles_to("pending"));
        table.park(2, Err("settled and untaken".to_owned()));
        table.next_id.set(1);

        assert_eq!(first, 1);
        assert_eq!(table.register(settles_to("third")), 3);
    }

    #[test]
    fn a_value_is_taken_once_and_a_rejection_is_taken_verbatim() {
        let table = FutureTable::new();
        table.park(1, Ok(HostValue::Number(42.0)));
        table.park(2, Err("the fetcher returned a stylesheet".to_owned()));

        assert_eq!(table.take(1), Ok(HostValue::Number(42.0)));
        assert_eq!(table.take(1), Err("future 1 has not settled".to_owned()));
        assert_eq!(
            table.take(2),
            Err("the fetcher returned a stylesheet".to_owned())
        );
    }

    #[test]
    fn a_timeout_is_a_deadline_and_anything_else_is_no_deadline_at_all() {
        assert!(deadline_from(f64::INFINITY).is_none());
        assert!(deadline_from(f64::NAN).is_none());
        let now = ClockInstant::now();
        let passed = deadline_from(-5.0).expect("a negative timeout is a deadline that has passed");
        assert!(passed <= ClockInstant::now());
        assert!(deadline_from(50.0).expect("a deadline") >= now + Duration::from_millis(50));
        assert!(deadline_from(f64::MAX).is_some());
    }
}
