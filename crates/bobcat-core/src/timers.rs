//! A realm's timers: the schedule, the two members the realm arms them
//! through, and the loop that fires what is due.
//!
//! Not the main thread's, and not a worker's — *a realm's*. Both kinds have
//! `setTimeout`, both clamp nesting the same way, and neither owns any of
//! this, so it lives beside [`crate::clock`] where both can reach it.
//!
//! Waiting is not here either. `bobcat-main` hands a view's deadline to that
//! view's painter and the host waits it out; `bobcat-workers` has no painter
//! to hand one to and waits out its own. What is common is only *when* a
//! timer is due, which is what [`TimerSchedule::next_deadline`] answers.
//!
//! `setTimeout` and `setInterval` split across the one boundary the realm
//! has. The callback stays in the realm, because no host value could hold
//! one; the schedule stays here, because the thread that waits is the thread
//! that owns the clock. What crosses is an id, and — when a timer comes due —
//! one call back into the realm module that filed the callback under it.
//!
//! Nothing here runs a callback or touches the document. It answers two
//! questions and only those: when the earliest arming comes due, and which
//! ids a round found due.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::rc::Rc;
use std::time::Duration;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::clock::ClockInstant;
use crate::esm::{HOST_MODULE_SPECIFIER, TIMER_MODULE_SPECIFIER};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

/// Timers that come due in one round without touching the heap. A card with
/// more than this many deadlines inside one millisecond is unusual.
const INLINE_DUE_TIMERS: usize = 4;

/// The nesting level past which HTML's timer initialization steps stop
/// honoring a short timeout, and the floor they clamp it to.
///
/// This is what keeps a `setTimeout(f, 0)` that re-arms itself from spinning
/// this thread: the chain runs unclamped to that depth and waits from there
/// on, which is the behavior a card compiled against a browser is written
/// for.
const CLAMPED_NESTING: u32 = 5;
const CLAMPED_DELAY: Duration = Duration::from_millis(4);

/// The longest delay HTML's `long` timeout names, in milliseconds — about
/// 24 days, and short enough that no deadline built from it can overflow the
/// clock.
const MAX_DELAY_MILLISECONDS: f64 = 2_147_483_647.0;

/// One armed timer.
struct Timer {
    /// When it next comes due.
    deadline: ClockInstant,
    /// The delay the realm asked for, before clamping: a repeat re-derives
    /// its next delay from this and its new nesting level, so the clamp
    /// cannot compound.
    requested: Duration,
    /// Whether it re-arms itself — `setInterval` rather than `setTimeout`.
    repeats: bool,
    /// The nesting level of the task this arming will run.
    nesting: u32,
}

/// One timer that has come due.
pub(crate) struct DueTimer {
    pub(crate) id: u32,
    /// The level its callback runs at, and so the level a timer that callback
    /// starts inherits.
    pub(crate) nesting: u32,
}

/// Every armed timer, in the order they come due.
pub(crate) struct TimerSchedule {
    armed: FxHashMap<u32, Timer>,
    /// Every arming ever pushed, ordered by `(deadline, id)` — which is the
    /// order the standard fires them in, id breaking a tie because an id is
    /// handed out in arming order. An entry the map no longer agrees with is
    /// a cleared or re-armed timer's leftover, and is dropped when it
    /// surfaces rather than searched for when it goes stale.
    order: BinaryHeap<Reverse<(ClockInstant, u32)>>,
    next_id: u32,
}

impl TimerSchedule {
    pub(crate) fn new() -> Self {
        Self {
            armed: FxHashMap::default(),
            order: BinaryHeap::new(),
            next_id: 1,
        }
    }

    /// Arms one timer and returns the id the realm files its callback under.
    ///
    /// `nesting` is the level the caller runs at — zero outside a timer
    /// callback, the running timer's level inside one. It decides both the
    /// clamp this arming gets and the level the task it schedules will run
    /// at, exactly as HTML's timer initialization steps do.
    pub(crate) fn arm(
        &mut self,
        delay_milliseconds: f64,
        repeats: bool,
        nesting: u32,
        now: ClockInstant,
    ) -> u32 {
        let id = self.allocate_id();
        let requested = requested_delay(delay_milliseconds);
        self.insert(
            id,
            Timer {
                deadline: now + effective_delay(requested, nesting),
                requested,
                repeats,
                nesting: nesting.saturating_add(1),
            },
        );
        id
    }

    /// Disarms a timer, whether or not one is armed under that id.
    pub(crate) fn clear(&mut self, id: u32) {
        self.armed.remove(&id);
    }

    /// The earliest deadline still armed, dropping leftovers on the way.
    pub(crate) fn next_deadline(&mut self) -> Option<ClockInstant> {
        loop {
            let Reverse((deadline, id)) = *self.order.peek()?;
            if self
                .armed
                .get(&id)
                .is_some_and(|timer| timer.deadline == deadline)
            {
                return Some(deadline);
            }
            self.order.pop();
        }
    }

    /// Takes every timer due at `now`, in fire order, re-arming the repeating
    /// ones as it goes.
    ///
    /// A repeat is re-armed here rather than after its callback returns, so
    /// that clearing it *from* that callback removes the next arming as well.
    /// The re-arming is held back until the batch is closed, because a repeat
    /// whose delay rounds to nothing would otherwise come due inside the very
    /// batch it is being taken from.
    pub(crate) fn take_due(
        &mut self,
        now: ClockInstant,
    ) -> SmallVec<[DueTimer; INLINE_DUE_TIMERS]> {
        let mut due = SmallVec::new();
        let mut repeating: SmallVec<[(u32, Timer); INLINE_DUE_TIMERS]> = SmallVec::new();
        while let Some(&Reverse((deadline, id))) = self.order.peek() {
            if deadline > now {
                break;
            }
            self.order.pop();
            // Cleared since it was pushed, or superseded by a later arming of
            // the same id: either way this entry names no live deadline.
            let Some(timer) = self.armed.get(&id) else {
                continue;
            };
            if timer.deadline != deadline {
                continue;
            }
            let nesting = timer.nesting;
            if timer.repeats {
                let requested = timer.requested;
                repeating.push((
                    id,
                    Timer {
                        deadline: now + effective_delay(requested, nesting),
                        requested,
                        repeats: true,
                        nesting: nesting.saturating_add(1),
                    },
                ));
            } else {
                self.armed.remove(&id);
            }
            due.push(DueTimer { id, nesting });
        }
        for (id, timer) in repeating {
            self.insert(id, timer);
        }
        due
    }

    fn insert(&mut self, id: u32, timer: Timer) {
        self.order.push(Reverse((timer.deadline, id)));
        self.armed.insert(id, timer);
    }

    /// The next id no armed timer holds.
    ///
    /// Ids start at one and never wrap onto a live timer, because a card
    /// stores one and tests it for truth before clearing it.
    fn allocate_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            if id != 0 && !self.armed.contains_key(&id) {
                return id;
            }
        }
    }
}

/// A delay as the realm asked for it, put through HTML's `long` conversion:
/// anything non-finite is zero, a negative delay is zero, and the rest is
/// truncated to whole milliseconds and capped.
fn requested_delay(milliseconds: f64) -> Duration {
    let clamped = if milliseconds.is_finite() {
        milliseconds.clamp(0.0, MAX_DELAY_MILLISECONDS).trunc()
    } else {
        0.0
    };
    Duration::from_secs_f64(clamped / 1_000.0)
}

/// The delay a timer armed at `nesting` actually waits.
fn effective_delay(requested: Duration, nesting: u32) -> Duration {
    if nesting > CLAMPED_NESTING && requested < CLAMPED_DELAY {
        CLAMPED_DELAY
    } else {
        requested
    }
}

/// Called on `bobcat:timers` when a timer this schedule armed comes due.
const TIMER_RUN_EXPORT: &str = "__BobcatRunTimer";

/// The timer schedule, and the nesting level the next timer inherits.
///
/// Shared with the host members that maintain it, so it is `Rc`: the native
/// `setTimer` export and the loop that fires what it armed are different
/// stack frames on the same thread.
pub(crate) struct TimerState {
    schedule: RefCell<TimerSchedule>,
    /// HTML's timer nesting level — zero outside a timer's callback, and the
    /// running timer's level inside one. It is what a timer started from a
    /// timer inherits, and so what decides when a chain of them stops being
    /// allowed to ask for no delay at all.
    nesting: Cell<u32>,
}

impl TimerState {
    pub(crate) fn new() -> Self {
        Self {
            schedule: RefCell::new(TimerSchedule::new()),
            nesting: Cell::new(0),
        }
    }

    /// The earliest deadline armed here.
    ///
    /// Not a pure question: a cleared or re-armed timer leaves its old heap
    /// entry behind, and asking is what discards them.
    pub(crate) fn next_deadline(&self) -> Option<ClockInstant> {
        self.schedule.borrow_mut().next_deadline()
    }
}

/// Runs every timer due now in one realm, in the order the standard fires
/// them, and answers with whatever their callbacks threw.
///
/// `None` means nothing was due, which is what lets a caller with per-batch
/// bookkeeping — `bobcat-main`, which counts removals toward a collection —
/// tell an empty batch from a batch that threw nothing.
///
/// A timer is its own task, so one that throws neither stops the ones behind
/// it nor ends the realm: the same standing an event listener that throws
/// already has.
pub(crate) fn run_due_timers(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    timers: &TimerState,
) -> Option<Vec<ScriptError>> {
    let due = timers.schedule.borrow_mut().take_due(ClockInstant::now());
    if due.is_empty() {
        return None;
    }
    let mut failures = Vec::new();
    for timer in due {
        // The level a timer this callback starts inherits. Restored around
        // every call, thrown or not, because the next one in the batch is not
        // nested inside this one.
        timers.nesting.set(timer.nesting);
        let ran = engine.call_module_export(
            js_runtime,
            TIMER_MODULE_SPECIFIER,
            TIMER_RUN_EXPORT,
            &[HostArgument::Number(f64::from(timer.id))],
        );
        timers.nesting.set(0);
        if let Err(error) = ran {
            failures.push(error);
        }
    }
    Some(failures)
}

/// Installs the two members a realm's timers speak to.
///
/// Neither runs a callback or touches a document: one arms a deadline and
/// answers with the id it armed, the other disarms one. Firing is the owning
/// thread's, through [`run_due_timers`].
pub(crate) fn install_timer_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    timers: &Rc<TimerState>,
) -> Result<(), ScriptError> {
    let state = Rc::clone(timers);
    install(engine, js_runtime, "setTimer", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.setTimer";
        let delay = number_argument(NAME, arguments, 0)?;
        let repeats = boolean_argument(NAME, arguments, 1)?;
        let id = state.schedule.borrow_mut().arm(
            delay,
            repeats,
            state.nesting.get(),
            ClockInstant::now(),
        );
        Ok(HostValue::Number(f64::from(id)))
    })?;

    let state = Rc::clone(timers);
    install(engine, js_runtime, "clearTimer", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.clearTimer";
        let id = timer_id_argument(NAME, arguments, 0)?;
        state.schedule.borrow_mut().clear(id);
        Ok(HostValue::Undefined)
    })
}

fn install(
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

fn argument(arguments: &[HostValue], index: usize) -> &HostValue {
    arguments.get(index).unwrap_or(&HostValue::Undefined)
}

fn number_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<f64, String> {
    match *argument(arguments, index) {
        HostValue::Number(value) => Ok(value),
        _ => Err(format!("{function} expects a number for argument {index}")),
    }
}

fn boolean_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match *argument(arguments, index) {
        HostValue::Boolean(value) => Ok(value),
        _ => Err(format!("{function} expects a boolean for argument {index}")),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the integer and range checks above make the value a representable id"
)]
fn timer_id_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<u32, String> {
    let value = number_argument(function, arguments, index)?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
        return Err(format!(
            "{function} expects a timer id for argument {index}"
        ));
    }
    Ok(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(due: &[DueTimer]) -> Vec<u32> {
        due.iter().map(|timer| timer.id).collect()
    }

    #[test]
    fn ids_start_at_one_so_a_card_can_test_one_for_truth() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        assert_eq!(schedule.arm(0.0, false, 0, now), 1);
        assert_eq!(schedule.arm(0.0, false, 0, now), 2);
    }

    #[test]
    fn timers_due_together_fire_in_the_order_they_were_armed() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        let first = schedule.arm(0.0, false, 0, now);
        let second = schedule.arm(0.0, false, 0, now);

        assert_eq!(ids(&schedule.take_due(now)), vec![first, second]);
    }

    #[test]
    fn a_later_deadline_waits_however_it_was_armed() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        let late = schedule.arm(50.0, false, 0, now);
        let soon = schedule.arm(0.0, false, 0, now);

        assert_eq!(ids(&schedule.take_due(now)), vec![soon]);
        assert_eq!(
            schedule.next_deadline(),
            Some(now + Duration::from_millis(50))
        );
        assert_eq!(
            ids(&schedule.take_due(now + Duration::from_millis(50))),
            vec![late]
        );
    }

    #[test]
    fn a_one_shot_is_taken_once_and_leaves_nothing_behind() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        schedule.arm(0.0, false, 0, now);

        assert_eq!(schedule.take_due(now).len(), 1);
        assert!(schedule.take_due(now).is_empty());
        assert_eq!(schedule.next_deadline(), None);
    }

    #[test]
    fn a_repeat_re_arms_but_never_inside_the_batch_that_took_it() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        let id = schedule.arm(0.0, true, 0, now);

        assert_eq!(ids(&schedule.take_due(now)), vec![id]);
        assert_eq!(ids(&schedule.take_due(now)), vec![id]);
        assert_eq!(ids(&schedule.take_due(now)), vec![id]);
    }

    #[test]
    fn a_repeat_that_asked_for_nothing_is_clamped_once_it_nests_deeply() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        schedule.arm(0.0, true, 0, now);
        // Levels one through five run as asked; the sixth and everything
        // after it waits the floor.
        for _ in 0..CLAMPED_NESTING {
            assert_eq!(schedule.take_due(now).len(), 1);
            assert_eq!(schedule.next_deadline(), Some(now));
        }
        assert_eq!(schedule.take_due(now).len(), 1);
        assert_eq!(schedule.next_deadline(), Some(now + CLAMPED_DELAY));
    }

    #[test]
    fn a_cleared_timer_never_comes_due() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        let id = schedule.arm(0.0, true, 0, now);
        schedule.clear(id);

        assert!(schedule.take_due(now).is_empty());
        assert_eq!(schedule.next_deadline(), None);
    }

    #[test]
    fn clearing_a_repeat_after_it_was_taken_removes_the_arming_it_left() {
        let mut schedule = TimerSchedule::new();
        let now = ClockInstant::now();

        let id = schedule.arm(0.0, true, 0, now);
        assert_eq!(schedule.take_due(now).len(), 1);
        schedule.clear(id);

        assert!(schedule.take_due(now).is_empty());
        assert_eq!(schedule.next_deadline(), None);
    }

    #[test]
    fn a_delay_that_is_not_a_delay_is_no_delay() {
        assert_eq!(requested_delay(f64::NAN), Duration::ZERO);
        assert_eq!(requested_delay(f64::NEG_INFINITY), Duration::ZERO);
        assert_eq!(requested_delay(f64::INFINITY), Duration::ZERO);
        assert_eq!(requested_delay(-5.0), Duration::ZERO);
        assert_eq!(requested_delay(1.9), Duration::from_millis(1));
        assert_eq!(
            requested_delay(MAX_DELAY_MILLISECONDS * 4.0),
            Duration::from_millis(2_147_483_647)
        );
    }

    #[test]
    fn the_clamp_only_reaches_delays_shorter_than_the_floor() {
        assert_eq!(
            effective_delay(Duration::ZERO, CLAMPED_NESTING),
            Duration::ZERO
        );
        assert_eq!(
            effective_delay(Duration::ZERO, CLAMPED_NESTING + 1),
            CLAMPED_DELAY
        );
        assert_eq!(
            effective_delay(Duration::from_millis(50), CLAMPED_NESTING + 1),
            Duration::from_millis(50)
        );
    }
}
