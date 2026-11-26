//! A realm's timers: the schedule, the two members the realm arms them
//! through, and the loop that fires what is due.
//!
//! Not the main thread's, and not a worker's — *a realm's*. Both kinds have
//! `setTimeout`, both clamp nesting the same way, and neither owns any of
//! this, so it lives beside [`crate::clock`] where both can reach it.
//!
//! Waiting is not here either. Each realm has a task of its own that waits
//! its deadlines out — a view's on `bobcat-main`, a worker's on
//! `bobcat-workers` — through [`crate::clock::sleep_until`], and no deadline
//! crosses a thread or reaches a host at all. Nor is firing: when a deadline
//! passes, that realm's epilogue is what runs what came due. What is common
//! here is only *when* a timer is due, which is what
//! [`TimerSchedule::next_deadline`] answers.
//!
//! `setTimeout` and `setInterval` split across the one boundary the realm
//! has. The callback stays in the realm, because no host value could hold
//! one; the schedule stays here, because the task that waits is the task
//! that owns the clock. What crosses is an id, and — when a timer comes due —
//! one call back into the realm module that filed the callback under it.
//!
//! Nothing here runs a callback or touches the document. It answers two
//! questions and only those: when the earliest arming comes due, and which
//! ids have come due now.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::rc::Rc;
use std::time::Duration;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::clock::ClockInstant;
use crate::esm::{Export, HOST_MODULE_SPECIFIER};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

const INLINE_DUE_TIMERS: usize = 4;

const CLAMPED_NESTING: u32 = 5;
const CLAMPED_DELAY: Duration = Duration::from_millis(4);

const MAX_DELAY_MILLISECONDS: f64 = 2_147_483_647.0;

struct Timer {
    deadline: ClockInstant,
    requested: Duration,
    repeats: bool,
    nesting: u32,
}

pub(crate) struct DueTimer {
    pub(crate) id: u32,
    pub(crate) nesting: u32,
}

pub(crate) struct TimerSchedule {
    armed: FxHashMap<u32, Timer>,
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

    pub(crate) fn is_armed(&self) -> bool {
        !self.armed.is_empty()
    }

    pub(crate) fn clear(&mut self, id: u32) {
        self.armed.remove(&id);
    }

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

fn requested_delay(milliseconds: f64) -> Duration {
    let clamped = if milliseconds.is_finite() {
        milliseconds.clamp(0.0, MAX_DELAY_MILLISECONDS).trunc()
    } else {
        0.0
    };
    Duration::from_secs_f64(clamped / 1_000.0)
}

fn effective_delay(requested: Duration, nesting: u32) -> Duration {
    if nesting > CLAMPED_NESTING && requested < CLAMPED_DELAY {
        CLAMPED_DELAY
    } else {
        requested
    }
}

pub(crate) struct TimerState {
    schedule: RefCell<TimerSchedule>,
    nesting: Cell<u32>,
}

impl TimerState {
    pub(crate) fn new() -> Self {
        Self {
            schedule: RefCell::new(TimerSchedule::new()),
            nesting: Cell::new(0),
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<ClockInstant> {
        self.schedule.borrow_mut().next_deadline()
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.schedule.borrow().is_armed()
    }
}

pub(crate) fn run_due_timers(
    engine: &ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    timers: &TimerState,
) -> Option<Vec<ScriptError>> {
    let due = {
        let mut schedule = timers.schedule.borrow_mut();
        if !schedule.is_armed() {
            return None;
        }
        schedule.take_due(ClockInstant::now())
    };
    if due.is_empty() {
        return None;
    }
    let mut failures = Vec::new();
    for timer in due {
        timers.nesting.set(timer.nesting);
        let ran = engine.call_module_export(
            js_runtime,
            Export::RunTimer,
            &[HostArgument::Number(f64::from(timer.id))],
        );
        timers.nesting.set(0);
        if let Err(error) = ran {
            failures.push(error);
        }
    }
    Some(failures)
}

pub(crate) fn install_timer_members(
    engine: &mut ScriptEngine,
    timers: &Rc<TimerState>,
) -> Result<(), ScriptError> {
    let state = Rc::clone(timers);
    install(engine, "setTimer", 2, move |arguments| {
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
    install(engine, "clearTimer", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.clearTimer";
        let id = timer_id_argument(NAME, arguments, 0)?;
        state.schedule.borrow_mut().clear(id);
        Ok(HostValue::Undefined)
    })
}

fn install(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostArgument<'_>]) -> Result<HostValue, String> + 'static,
) -> Result<(), ScriptError> {
    engine.register_host_module_function(HOST_MODULE_SPECIFIER, name, arity, callback)
}

fn argument<'a>(arguments: &[HostArgument<'a>], index: usize) -> HostArgument<'a> {
    arguments
        .get(index)
        .copied()
        .unwrap_or(HostArgument::Undefined)
}

fn number_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<f64, String> {
    match argument(arguments, index) {
        HostArgument::Number(value) => Ok(value),
        _ => Err(format!("{function} expects a number for argument {index}")),
    }
}

fn boolean_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<bool, String> {
    match argument(arguments, index) {
        HostArgument::Boolean(value) => Ok(value),
        _ => Err(format!("{function} expects a boolean for argument {index}")),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the integer and range checks above make the value a representable id"
)]
fn timer_id_argument(
    function: &str,
    arguments: &[HostArgument<'_>],
    index: usize,
) -> Result<u32, String> {
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
