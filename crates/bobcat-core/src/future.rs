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
use crate::esm::{FUTURE_MODULE_SPECIFIER, HOST_MODULE_SPECIFIER};
use crate::jobs::JsThreadHandle;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

/// The longest timeout a `wait` can name, in milliseconds — HTML's own
/// `long` ceiling, about 24 days, and short enough that no deadline built
/// from it can overflow the clock.
const MAX_TIMEOUT_MILLISECONDS: f64 = 2_147_483_647.0;

/// Called on `bobcat:future` when a future the realm asked to settle
/// asynchronously has settled.
const SETTLE_EXPORT: &str = "__BobcatSettleFuture";

/// One future's answer: the value it settled to, or why it will not settle.
///
/// The value is a [`HostValue`], which is the whole of what the boundary
/// carries: an operation whose product is not one of those has no business
/// being a `Future`.
pub(crate) type Outcome = Result<HostValue, String>;

/// One registered operation, as the table holds it.
///
/// `!Send` and boxed: it is awaited on the engine thread that registered it,
/// either inside a job's wait or on a task the owner's epilogue spawned.
pub(crate) type HostFuture = Pin<Box<dyn Future<Output = Outcome>>>;

/// Every future one realm has registered and not finished with.
///
/// Shared with the three host members that maintain it, so it is `Rc`: the
/// members and the epilogue that drains [`Self::settle_requests`] are
/// different stack frames on the same thread.
pub(crate) struct FutureTable {
    next_id: Cell<u32>,
    /// Registered, and neither waited on nor asked to settle. A timed-out
    /// wait puts its future back here.
    pending: RefCell<FxHashMap<u32, HostFuture>>,
    /// An outcome a wait parked, until the realm takes it.
    settled: RefCell<FxHashMap<u32, Outcome>>,
    /// The futures a `.then` asked to settle asynchronously. The owner's
    /// epilogue drains this and spawns a task per entry.
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

    /// Files one operation and answers with the id the realm names it by.
    #[allow(
        dead_code,
        reason = "no production future is registered yet; the test producer below and the coming lynx.fetchBundle are its callers"
    )]
    pub(crate) fn register(&self, future: impl Future<Output = Outcome> + 'static) -> u32 {
        let id = self.allocate_id();
        self.pending.borrow_mut().insert(id, Box::pin(future));
        id
    }

    /// The futures a `.then` asked to settle since the last drain.
    pub(crate) fn take_settle_requests(&self) -> Vec<(u32, HostFuture)> {
        std::mem::take(&mut *self.settle_requests.borrow_mut())
    }

    /// Parks one outcome until the realm takes it.
    fn park(&self, id: u32, outcome: Outcome) {
        self.settled.borrow_mut().insert(id, outcome);
    }

    /// The parked outcome, as the realm reads it: a rejection reaches the
    /// realm as the host's own wording, and the class it becomes there is the
    /// realm's to choose.
    fn take(&self, id: u32) -> Result<HostValue, String> {
        match self.settled.borrow_mut().remove(&id) {
            Some(outcome) => outcome,
            None => Err(format!("future {id} has not settled")),
        }
    }

    /// The next id no live future holds.
    ///
    /// Ids start at one and never wrap onto a live future, for the reason a
    /// timer id does not: the realm stores one and tests it for truth.
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

/// What one synchronous wait ended on.
enum Waited {
    /// The realm ended under it: the token, the biased first arm.
    Ended,
    /// The deadline passed. The operation is untouched and still running.
    TimedOut,
    Settled(Outcome),
}

/// Installs the members a realm's `Future` class speaks to.
///
/// None of them runs a callback or touches a document: one parks this job on
/// an answer, one takes an answer that has arrived, and one asks for the
/// asynchronous path instead.
pub(crate) fn install(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    table: &Rc<FutureTable>,
    token: CancellationToken,
    thread: JsThreadHandle,
) -> Result<(), ScriptError> {
    let futures = Rc::clone(table);
    install_member(engine, js_runtime, "waitFuture", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.waitFuture";
        let id = future_id_argument(NAME, arguments, 0)?;
        let deadline = deadline_from(number_argument(NAME, arguments, 1)?);
        let Some(mut future) = futures.pending.borrow_mut().remove(&id) else {
            return Err(format!("future {id} is not pending"));
        };
        let waited = thread.wait(async {
            // Built here rather than in job context: the free tokio timer
            // entry points read the ambient runtime, and a job has none.
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
                // Nothing was cancelled: the operation is still running, and
                // a later wait or a `.then` still answers from it.
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
    install_member(engine, js_runtime, "takeFuture", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.takeFuture";
        let id = future_id_argument(NAME, arguments, 0)?;
        futures.take(id)
    })?;

    let futures = Rc::clone(table);
    install_member(engine, js_runtime, "settleFuture", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.settleFuture";
        let id = future_id_argument(NAME, arguments, 0)?;
        let Some(future) = futures.pending.borrow_mut().remove(&id) else {
            return Err(format!("future {id} is not pending"));
        };
        futures.settle_requests.borrow_mut().push((id, future));
        Ok(HostValue::Undefined)
    })?;

    // The one thing that registers a future today. Nothing in production does
    // yet — the table is infrastructure, and `lynx.fetchBundle` is what will
    // fill it — so the end-to-end tests of both realm kinds bring a producer
    // of their own: `testFuture(delayMs, value, rejects)` files an operation
    // that settles to that string, or rejects with it, once the delay has
    // passed, and answers with its id.
    #[cfg(test)]
    {
        let futures = Rc::clone(table);
        install_member(engine, js_runtime, "testFuture", 3, move |arguments| {
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

/// Hands one settled outcome back to the realm that asked for it
/// asynchronously.
pub(crate) fn deliver(
    engine: &mut ScriptEngine,
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
            FUTURE_MODULE_SPECIFIER,
            SETTLE_EXPORT,
            &[
                HostArgument::Number(f64::from(id)),
                HostArgument::Boolean(rejected),
                value.as_argument(),
            ],
        )
        .map(|_| ())
}

/// When a wait of `milliseconds` gives up, or `None` for one that never does.
///
/// Non-finite is no deadline at all — which is what `Future.wait()` with no
/// argument passes — and a negative one is a deadline that has already
/// passed. The ceiling is HTML's `long`, so no arithmetic here can overflow
/// the clock.
fn deadline_from(milliseconds: f64) -> Option<ClockInstant> {
    if !milliseconds.is_finite() {
        return None;
    }
    let clamped = milliseconds.clamp(0.0, MAX_TIMEOUT_MILLISECONDS);
    Some(ClockInstant::now() + Duration::from_secs_f64(clamped / 1_000.0))
}

fn install_member(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostValue]) -> Result<HostValue, String> + 'static,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(
        js_runtime,
        HOST_MODULE_SPECIFIER,
        name,
        arity,
        Box::new(callback),
    )
}

fn number_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<f64, String> {
    match arguments.get(index) {
        Some(&HostValue::Number(value)) => Ok(value),
        _ => Err(format!("{function} expects a number for argument {index}")),
    }
}

#[cfg(test)]
fn string_argument<'a>(
    function: &str,
    arguments: &'a [HostValue],
    index: usize,
) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}

#[cfg(test)]
fn boolean_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match arguments.get(index) {
        Some(&HostValue::Boolean(value)) => Ok(value),
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
    arguments: &[HostValue],
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
        // Wrapped onto the two ids above, both of which are live.
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
        // Clamped rather than overflowing the clock.
        assert!(deadline_from(f64::MAX).is_some());
    }
}
