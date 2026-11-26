//! Wasm's substitute for a time driver: one thread that owns a heap of
//! deadlines and wakes whoever is waiting on each.
//!
//! tokio's timer reads `std::time::Instant`, which panics on
//! wasm32-unknown-unknown, so no engine thread there can enable one. What is
//! left is what a driver is made of anyway — a sorted set of deadlines and a
//! wait that ends at the earliest of them — and a Worker can do that with
//! `park_timeout`, which takes a duration rather than an instant and so needs
//! no clock the platform does not have.
//!
//! One thread for the whole process, never joined. It owns no realm, no
//! runtime and no view: a registration is a deadline and a slot to wake
//! through, and waking one is legal from any thread.
//!
//! # Why a slot rather than a bare waker
//!
//! A registration outlives the poll that made it, so the two facts a driver
//! owes have to be readable from both ends of it. The state flag is what the
//! woken task reads instead of the clock — the thread's clock is aligned to
//! its starter's to within half a round trip ([`crate::clock`]), not exactly,
//! so a wake can land while the task's own reading of the time is still
//! behind the deadline, and a future that trusted the clock there would
//! return `Pending` without re-registering and never be woken again. The
//! cancellation the flag also carries is what keeps the heap bounded: a
//! `Sleep` that is dropped un-fired — which is every re-armed wait in the
//! engine — takes its entry out of the thread's next sweep rather than leaving
//! it to fire into nothing.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering as Atomic};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::task::{Context, Poll, Waker};
use std::thread;

use terminus::link::clock::{AnswerWhenReady, alignment_channel};

use crate::clock::{ALIGNMENT_PATIENCE, ClockInstant};
use crate::view::EngineError;

const ARMED: u8 = 0;
const FIRED: u8 = 1;
const CANCELLED: u8 = 2;

struct AlarmSlot {
    state: AtomicU8,
    waker: Mutex<Option<Waker>>,
}

impl AlarmSlot {
    fn state(&self) -> u8 {
        self.state.load(Atomic::Acquire)
    }

    fn fire(&self) -> Option<Waker> {
        self.state
            .compare_exchange(ARMED, FIRED, Atomic::AcqRel, Atomic::Relaxed)
            .ok()
            .and_then(|_| lock(&self.waker).take())
    }
}

struct Alarm {
    deadline: ClockInstant,
    slot: Arc<AlarmSlot>,
}

impl PartialEq for Alarm {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}

impl Eq for Alarm {}

impl PartialOrd for Alarm {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Alarm {
    fn cmp(&self, other: &Self) -> Ordering {
        other.deadline.cmp(&self.deadline)
    }
}

#[derive(Default)]
struct Registry {
    arrivals: Mutex<Arrivals>,
    cancelled: AtomicUsize,
}

#[derive(Default)]
struct Arrivals {
    alarms: Vec<Alarm>,
    thread: Option<thread::Thread>,
}

static ALARMS: OnceLock<Registry> = OnceLock::new();

static STARTED: Mutex<bool> = Mutex::new(false);

pub(crate) fn start() -> Result<Option<AnswerWhenReady>, EngineError> {
    let mut started = lock(&STARTED);
    if *started {
        return Ok(None);
    }
    let registry = registrations();
    let (clock, clock_end) = alignment_channel();
    wasm_thread::Builder::new()
        .name("bobcat-alarm".to_owned())
        .spawn(move || {
            clock_end.align_to_spawner();
            run(registry);
        })
        .map_err(|error| EngineError::Thread {
            name: "alarm",
            message: error.to_string(),
        })?;
    *started = true;
    Ok(Some(clock.answer(ALIGNMENT_PATIENCE)))
}

fn registrations() -> &'static Registry {
    ALARMS.get_or_init(Registry::default)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|error| panic!("the alarm registry is poisoned: {error}"))
}

fn register(deadline: ClockInstant, waker: Waker) -> Arc<AlarmSlot> {
    let slot = Arc::new(AlarmSlot {
        state: AtomicU8::new(ARMED),
        waker: Mutex::new(Some(waker)),
    });
    let registry = registrations();
    let waiting = {
        let mut arrivals = lock(&registry.arrivals);
        arrivals.alarms.push(Alarm {
            deadline,
            slot: Arc::clone(&slot),
        });
        arrivals.thread.clone()
    };
    if let Some(thread) = waiting {
        thread.unpark();
    }
    slot
}

fn run(registry: &'static Registry) {
    lock(&registry.arrivals).thread = Some(thread::current());
    let mut pending: BinaryHeap<Alarm> = BinaryHeap::new();
    loop {
        pending.extend(std::mem::take(&mut lock(&registry.arrivals).alarms));
        let now = ClockInstant::now();
        while pending.peek().is_some_and(|alarm| alarm.deadline <= now) {
            if let Some(waker) = pending
                .pop()
                .expect("the heap was just peeked at")
                .slot
                .fire()
            {
                waker.wake();
            }
        }
        sweep_cancelled(registry, &mut pending);
        match pending.peek() {
            Some(next) => thread::park_timeout(next.deadline.saturating_duration_since(now)),
            None => thread::park(),
        }
    }
}

fn sweep_cancelled(registry: &'static Registry, pending: &mut BinaryHeap<Alarm>) {
    let cancelled = registry.cancelled.load(Atomic::Relaxed);
    if cancelled <= pending.len() / 2 {
        return;
    }
    pending.retain(|alarm| alarm.slot.state() == ARMED);
    registry.cancelled.fetch_sub(cancelled, Atomic::Relaxed);
}

pub(crate) fn sleep_until(deadline: ClockInstant) -> Sleep {
    Sleep {
        deadline,
        slot: None,
    }
}

pub(crate) struct Sleep {
    deadline: ClockInstant,
    slot: Option<Arc<AlarmSlot>>,
}

impl Sleep {
    fn elapsed(&self) -> bool {
        ClockInstant::now() >= self.deadline
            || self.slot.as_ref().is_some_and(|slot| slot.state() == FIRED)
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.elapsed() {
            return Poll::Ready(());
        }
        match &this.slot {
            Some(slot) if slot.state() == ARMED => {
                let mut held = lock(&slot.waker);
                if held
                    .as_ref()
                    .is_none_or(|waker| !waker.will_wake(context.waker()))
                {
                    *held = Some(context.waker().clone());
                }
            }
            _ => this.slot = Some(register(this.deadline, context.waker().clone())),
        }
        if this.elapsed() {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        let Some(slot) = self.slot.take() else {
            return;
        };
        if slot
            .state
            .compare_exchange(ARMED, CANCELLED, Atomic::AcqRel, Atomic::Relaxed)
            .is_err()
        {
            return;
        }
        *lock(&slot.waker) = None;
        if let Some(registry) = ALARMS.get() {
            registry.cancelled.fetch_add(1, Atomic::Relaxed);
        }
    }
}
