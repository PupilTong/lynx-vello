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
//! woken task reads instead of the clock — a Worker's `performance.timeOrigin`
//! is its own, so a wake can land while the task's own reading of the time is
//! still behind the deadline, and a future that trusted the clock there would
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

use crate::clock::ClockInstant;

/// Waiting for its deadline, and the only state a registration is wakeable in.
const ARMED: u8 = 0;
/// The deadline passed and the alarm thread took the waker. Whoever polls
/// next is done, whatever their own clock says.
const FIRED: u8 = 1;
/// The `Sleep` was dropped before its deadline. The heap entry is dead weight
/// until the next sweep, and wakes nobody.
const CANCELLED: u8 = 2;

/// What one registration and the future that made it share.
struct AlarmSlot {
    state: AtomicU8,
    /// Taken by whichever side ends the wait, and replaced by a poll on a new
    /// task. `None` once the wake has been delivered or the wait cancelled.
    waker: Mutex<Option<Waker>>,
}

impl AlarmSlot {
    fn state(&self) -> u8 {
        self.state.load(Atomic::Acquire)
    }

    /// Claims the wait for the alarm thread and hands back who to wake.
    ///
    /// The flag is published before the waker is taken, so a task woken by it
    /// cannot observe the wake without also observing the fact behind it.
    fn fire(&self) -> Option<Waker> {
        self.state
            .compare_exchange(ARMED, FIRED, Atomic::AcqRel, Atomic::Relaxed)
            .ok()
            .and_then(|_| lock(&self.waker).take())
    }
}

/// One deadline and the slot waiting on it. Ordered by the deadline alone:
/// two alarms due at once are equivalent, whichever was registered first.
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
        // Reversed, so the binary heap's root is the earliest deadline.
        other.deadline.cmp(&self.deadline)
    }
}

/// What every engine thread hands the alarm thread, and the one number it
/// hands back.
#[derive(Default)]
struct Registry {
    arrivals: Mutex<Arrivals>,
    /// Registrations cancelled since the last sweep, counted rather than
    /// removed: the cancelling thread is a runtime's, and taking the lock and
    /// searching a heap on it to reclaim one entry would cost more than the
    /// entry does.
    cancelled: AtomicUsize,
}

#[derive(Default)]
struct Arrivals {
    /// Registered but not yet in the thread's own heap.
    alarms: Vec<Alarm>,
    /// Published by the alarm thread before its first pass, so a registration
    /// that beats it to the lock is still seen.
    thread: Option<thread::Thread>,
}

/// The registry, and the thread reading it.
///
/// Two statics rather than one so the thread is handed the registry
/// directly: it must never reach back through an initializer it may itself
/// be inside.
static ALARMS: OnceLock<Registry> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();

/// Boots the alarm thread.
///
/// Called with the group that will use it rather than left to the first timer
/// a card arms: starting a Worker is not free, and a page whose first
/// `setTimeout` paid for one would wait out a cold start it never asked for.
pub(crate) fn start() {
    let _registry = registrations();
}

fn registrations() -> &'static Registry {
    let registry = ALARMS.get_or_init(Registry::default);
    STARTED.get_or_init(|| {
        wasm_thread::Builder::new()
            .name("bobcat-alarm".to_owned())
            .spawn(move || run(registry))
            .expect("the alarm thread starts");
    });
    registry
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|error| panic!("the alarm registry is poisoned: {error}"))
}

/// Registers one deadline and hands back the slot it will be answered on.
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
    // Outside the lock: an unpark that arrives before the thread parks is
    // remembered, so the only ordering that matters is push-then-unpark.
    if let Some(thread) = waiting {
        thread.unpark();
    }
    slot
}

/// The alarm thread's whole body.
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

/// Drops the entries whose futures are gone, once enough of the heap is dead
/// to be worth walking it.
///
/// The threshold is what keeps this amortized: a sweep is linear in the heap,
/// so paying for one per cancellation would make a loop that re-arms every
/// round quadratic in its own timer horizon. The count is read before the
/// sweep and subtracted after it, so a cancellation racing this one is still
/// counted for the next.
fn sweep_cancelled(registry: &'static Registry, pending: &mut BinaryHeap<Alarm>) {
    let cancelled = registry.cancelled.load(Atomic::Relaxed);
    if cancelled <= pending.len() / 2 {
        return;
    }
    pending.retain(|alarm| alarm.slot.state() == ARMED);
    registry.cancelled.fetch_sub(cancelled, Atomic::Relaxed);
}

/// Resolves once `deadline` has passed.
pub(crate) fn sleep_until(deadline: ClockInstant) -> Sleep {
    Sleep {
        deadline,
        slot: None,
    }
}

pub(crate) struct Sleep {
    deadline: ClockInstant,
    /// The registration the alarm thread holds, once one has been made.
    slot: Option<Arc<AlarmSlot>>,
}

impl Sleep {
    /// Whether this wait is over: the clock says so, or the alarm thread does.
    ///
    /// The clock first, because a caller that arms an already-passed deadline
    /// — which is every timer the engine finds due — must resolve with no
    /// round trip at all. The flag second, because the clock alone is not
    /// enough: it is read here on the waiting Worker and there on the alarm
    /// thread, and the two `performance.timeOrigin`s they are relative to are
    /// not the same one.
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
            // Still armed: the thread already holds this registration, so all
            // a re-poll owes it is the waker of whoever is waiting now.
            Some(slot) if slot.state() == ARMED => {
                let mut held = lock(&slot.waker);
                if held
                    .as_ref()
                    .is_none_or(|waker| !waker.will_wake(context.waker()))
                {
                    *held = Some(context.waker().clone());
                }
            }
            // Never registered, or registered on a slot the thread has
            // already used up: either way this wait needs one of its own.
            _ => this.slot = Some(register(this.deadline, context.waker().clone())),
        }
        // The thread may have fired the slot while the waker above was being
        // replaced, taking a waker this poll has already superseded. Nothing
        // would wake this task again, so the flag is read once more here.
        if this.elapsed() {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

impl Drop for Sleep {
    /// A wait that is dropped before it fires releases its task at once: the
    /// waker goes, and the entry the thread still holds is marked for the
    /// next sweep.
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
