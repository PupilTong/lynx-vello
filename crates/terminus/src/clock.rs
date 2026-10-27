//! The one monotonic instant type every timestamp the engine takes is read
//! from — the painter's frame clock, a commit's timing marks
//! ([`crate::visual::timing`]), a presented frame's
//! ([`crate::painter::PresentedFrame`]), and every instant `bobcat-core`
//! reads — and the alignment that makes a reading on one engine thread
//! comparable with a reading on another.
//!
//! # One clock per process natively, one per Worker on wasm32
//!
//! Natively a [`ClockInstant`] is a `std::time::Instant`, which is one
//! monotonic clock for every thread of the process: a commit's marks on the
//! document's thread and a present's on the painter's subtract as they are.
//!
//! `std::time::Instant` panics on `wasm32-unknown-unknown`, so that target
//! reads `web_time::Instant`, which is the current Worker's
//! `performance.timeOrigin + performance.now()`. `timeOrigin` is a reading
//! of the system clock taken when that Worker was created, and `now()` is
//! monotonic from then, so two Workers disagree by a constant: how far the
//! system clock and the monotonic clock had drifted apart between their two
//! creations (NTP slewing, and the whole length of any sleep in between,
//! during which the monotonic clock stops and the system clock does not).
//!
//! # Alignment at thread start
//!
//! Each engine thread is therefore aligned once, as the first thing it runs,
//! to the thread that spawned it ([`alignment_channel`]): the spawned thread
//! reads its own clock, asks the spawner for the spawner's reading, reads its
//! own clock again when the answer arrives, and takes the spawner's reading
//! to have been made halfway between its own two (NTP's offset estimate,
//! RFC 5905 §8). The difference is a per-thread offset that every later
//! [`ClockInstant::now`] on that thread adds. The spawner answers with its own
//! aligned reading, so a thread spawned by an aligned thread is aligned to the
//! same clock, and every aligned thread reads the clock of the first spawner,
//! the embedder's thread. Before the timed exchange the spawned thread waits
//! until the spawner is blocked listening for it, so neither timed message
//! waits on an executor or an event loop: the estimate's error is at most
//! half the round trip, and a round trip between two threads that each wait
//! on a futex is microseconds.
//!
//! Natively the exchange still runs, so its channels and timeouts are tested
//! where tests run, but its offset is only measured — the spawner's answer
//! hands it back — and never applied: `now()` there is `Instant::now()` and
//! nothing else.

#[cfg(any(test, target_arch = "wasm32"))]
use std::cell::Cell;
use std::future::Future;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(target_arch = "wasm32")]
use web_time::Instant;

/// One reading of the engine's monotonic clock, comparable with a reading
/// taken on any other engine thread.
///
/// The arithmetic is `std::time::Instant`'s: subtracting a later instant
/// saturates to zero, and adding a duration past what the platform can
/// represent panics, as the `checked_*` methods report instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockInstant(Instant);

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// This thread's offset from its spawner's clock, in nanoseconds: zero
    /// until [`SpawnedClock::align_to_spawner`] measures it, and zero for good
    /// on a thread that is never aligned.
    static OFFSET: Cell<i64> = const { Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// What aligning this thread measured, for a test that asks on it.
    static ALIGNMENT: Cell<Option<Alignment>> = const { Cell::new(None) };
}

impl ClockInstant {
    /// The clock read now, with this thread's alignment applied.
    #[must_use]
    #[inline]
    pub fn now() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self(Instant::now())
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self(apply_offset(Instant::now(), OFFSET.with(Cell::get)))
        }
    }

    /// How long ago this instant was; zero for one that has not come yet.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        Self::now().saturating_duration_since(*self)
    }

    /// How long after `earlier` this instant is; zero when it is not after it.
    #[must_use]
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_duration_since(earlier.0)
    }

    /// How long after `earlier` this instant is; zero when it is not after it.
    #[must_use]
    pub fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_duration_since(earlier.0)
    }

    /// How long after `earlier` this instant is, or `None` when it is before
    /// it.
    #[must_use]
    pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
        self.0.checked_duration_since(earlier.0)
    }

    /// This instant moved `duration` later, or `None` past what the platform
    /// can represent.
    #[must_use]
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration).map(Self)
    }

    /// This instant moved `duration` earlier, or `None` before what the
    /// platform can represent.
    #[must_use]
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration).map(Self)
    }

    /// The `std::time::Instant` this is, for an API that takes one (tokio's
    /// timer). Native only: on wasm32 there is no `std` instant to be.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub const fn into_std(self) -> Instant {
        self.0
    }
}

impl Add<Duration> for ClockInstant {
    type Output = Self;

    fn add(self, duration: Duration) -> Self {
        self.checked_add(duration)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for ClockInstant {
    fn add_assign(&mut self, duration: Duration) {
        *self = *self + duration;
    }
}

impl Sub<Duration> for ClockInstant {
    type Output = Self;

    fn sub(self, duration: Duration) -> Self {
        self.checked_sub(duration)
            .expect("overflow when subtracting duration from instant")
    }
}

impl SubAssign<Duration> for ClockInstant {
    fn sub_assign(&mut self, duration: Duration) {
        *self = *self - duration;
    }
}

impl Sub for ClockInstant {
    type Output = Duration;

    /// How long after `earlier` this instant is; zero when it is not after it.
    fn sub(self, earlier: Self) -> Duration {
        self.saturating_duration_since(earlier)
    }
}

/// `raw` moved by `offset` nanoseconds (later for a positive offset), held at
/// the earliest or latest instant the platform represents rather than
/// failing.
#[cfg(any(target_arch = "wasm32", test))]
#[inline]
fn apply_offset(raw: Instant, offset: i64) -> Instant {
    let distance = Duration::from_nanos(offset.unsigned_abs());
    let moved = if offset >= 0 {
        raw.checked_add(distance)
    } else {
        raw.checked_sub(distance)
    };
    moved.unwrap_or_else(|| saturate(raw, distance, offset >= 0))
}

/// The farthest `raw` moves toward `distance` (later when `later`) without
/// leaving the platform's range: a bisection over the distance, reached
/// only by an offset that no two threads of one machine can be apart by.
#[cfg(any(target_arch = "wasm32", test))]
#[cold]
fn saturate(raw: Instant, distance: Duration, later: bool) -> Instant {
    let step = |by: Duration| {
        if later {
            raw.checked_add(by)
        } else {
            raw.checked_sub(by)
        }
    };
    // `step(fits)` is always `Some` and `step(fails)` always `None`.
    let (mut fits, mut fails) = (Duration::ZERO, distance);
    while fails.saturating_sub(fits) > Duration::from_nanos(1) {
        let middle = fits + fails.saturating_sub(fits) / 2;
        if step(middle).is_some() {
            fits = middle;
        } else {
            fails = middle;
        }
    }
    step(fits).unwrap_or(raw)
}

/// What aligning one thread to its spawner measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alignment {
    offset_nanos: i64,
    round_trip: Duration,
}

impl Alignment {
    /// How far the spawner's clock is ahead of this thread's own, in
    /// nanoseconds (negative when it is behind). On wasm32 every later
    /// [`ClockInstant::now`] on the thread adds it; natively it is measured
    /// and not applied, and is zero within the round trip.
    #[must_use]
    pub const fn offset_nanos(self) -> i64 {
        self.offset_nanos
    }

    /// From asking the spawner to having its answer, on this thread's own
    /// clock. The offset is right to within half of it.
    #[must_use]
    pub const fn round_trip(self) -> Duration {
        self.round_trip
    }
}

/// NTP's offset estimate from one exchange: `answered − (sent + received) / 2`,
/// where `sent` and `received` are the spawned thread's own readings around
/// the exchange and `answered` is the spawner's reading in between.
fn estimate(sent: Instant, answered: Instant, received: Instant) -> Alignment {
    let round_trip = received.saturating_duration_since(sent);
    // At most `received`, so always representable.
    let midpoint = sent.checked_add(round_trip / 2).unwrap_or(received);
    let offset_nanos = match answered.checked_duration_since(midpoint) {
        Some(ahead) => i64::try_from(ahead.as_nanos()).unwrap_or(i64::MAX),
        None => i64::try_from(midpoint.duration_since(answered).as_nanos())
            .map_or(i64::MIN, |behind| -behind),
    };
    Alignment {
        offset_nanos,
        round_trip,
    }
}

/// What aligning the current thread measured, or `None` on a thread that was
/// never aligned or whose spawner gave up.
#[cfg(test)]
fn thread_alignment() -> Option<Alignment> {
    ALIGNMENT.with(Cell::get)
}

/// Makes what aligning this thread measured the thread's own: on wasm32
/// applied to every later [`ClockInstant::now`]; natively nothing, beyond
/// the record a test reads.
fn install(alignment: Option<Alignment>) {
    #[cfg(test)]
    ALIGNMENT.with(|recorded| recorded.set(alignment));
    #[cfg(target_arch = "wasm32")]
    if let Some(alignment) = alignment {
        OFFSET.with(|offset| offset.set(alignment.offset_nanos));
    }
    #[cfg(not(any(test, target_arch = "wasm32")))]
    let _ = alignment;
}

/// Where one alignment exchange is. Each step is written by one end and
/// waited for by the other.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// Created; the spawned thread has not run yet.
    Spawned,
    /// The spawned thread is running and waits for the spawner to be ready to
    /// answer at once.
    Ready,
    /// The spawner is blocked waiting for the request, so its answer is not
    /// delayed by whatever else its thread would be doing.
    Listening,
    /// The spawned thread read its clock and waits for the answer.
    Requested,
    /// The spawner's reading, taken as the request arrived.
    Answered(Instant),
    /// The spawned thread's measurement, installed on it.
    Aligned(Alignment),
    /// One end gave up or went away before the exchange finished; the
    /// spawned thread runs unaligned.
    Abandoned,
}

#[derive(Debug)]
struct ExchangeState {
    step: Step,
    /// The spawner's task while it waits for [`Step::Ready`] without
    /// blocking its thread.
    waker: Option<Waker>,
}

#[derive(Debug)]
struct Exchange {
    state: Mutex<ExchangeState>,
    changed: Condvar,
}

impl Exchange {
    fn lock(&self) -> MutexGuard<'_, ExchangeState> {
        // Nothing panics while holding the lock, and a poisoned exchange is
        // still a valid one.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Moves to `step` and tells the other end.
    fn advance(&self, mut state: MutexGuard<'_, ExchangeState>, step: Step) {
        state.step = step;
        let waker = state.waker.take();
        drop(state);
        self.changed.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Blocks until `done` holds of the step, or until `deadline`, when
    /// there is one. Hands the lock back either way, with whether `done`
    /// holds.
    fn wait_for<'a>(
        &self,
        mut state: MutexGuard<'a, ExchangeState>,
        deadline: Option<Instant>,
        done: impl Fn(Step) -> bool,
    ) -> (MutexGuard<'a, ExchangeState>, bool) {
        loop {
            if done(state.step) {
                return (state, true);
            }
            state = match deadline {
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Some(deadline) => {
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        return (state, false);
                    };
                    self.changed
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
            };
        }
    }
}

/// The two ends of one thread's alignment exchange, made by the spawner
/// before it spawns: the [`SpawnedClock`] moves into the new thread, whose
/// first statement is [`SpawnedClock::align_to_spawner`], and the spawner
/// answers through the [`SpawnerClock`].
#[must_use]
pub fn alignment_channel() -> (SpawnerClock, SpawnedClock) {
    let exchange = Arc::new(Exchange {
        state: Mutex::new(ExchangeState {
            step: Step::Spawned,
            waker: None,
        }),
        changed: Condvar::new(),
    });
    (
        SpawnerClock {
            exchange: Arc::clone(&exchange),
        },
        SpawnedClock { exchange },
    )
}

/// The spawned thread's end of an alignment exchange.
#[derive(Debug)]
pub struct SpawnedClock {
    exchange: Arc<Exchange>,
}

impl SpawnedClock {
    /// Aligns this thread's clock to its spawner's, blocking until the
    /// spawner answers or gives up. The spawner's answer hands back what was
    /// measured; when the spawner went away or gave up first nothing was,
    /// and this thread runs on its own clock, as an unaligned thread does.
    pub fn align_to_spawner(self) {
        install(self.measure());
    }

    /// The exchange itself, from this thread's end.
    fn measure(&self) -> Option<Alignment> {
        let exchange = &self.exchange;
        let state = exchange.lock();
        if matches!(state.step, Step::Abandoned) {
            return None;
        }
        exchange.advance(state, Step::Ready);
        let (state, _) = exchange.wait_for(exchange.lock(), None, |step| {
            matches!(step, Step::Listening | Step::Abandoned)
        });
        if matches!(state.step, Step::Abandoned) {
            return None;
        }
        let sent = Instant::now();
        exchange.advance(state, Step::Requested);
        let (state, _) = exchange.wait_for(exchange.lock(), None, |step| {
            matches!(step, Step::Answered(_) | Step::Abandoned)
        });
        let received = Instant::now();
        let Step::Answered(answered) = state.step else {
            return None;
        };
        let alignment = estimate(sent, answered, received);
        exchange.advance(state, Step::Aligned(alignment));
        Some(alignment)
    }
}

impl Drop for SpawnedClock {
    /// A spawned thread that ends, or drops its end unused, tells the
    /// spawner rather than leaving it to wait out its patience.
    fn drop(&mut self) {
        let state = self.exchange.lock();
        if !matches!(state.step, Step::Aligned(_)) {
            self.exchange.advance(state, Step::Abandoned);
        }
    }
}

/// The spawner's end of an alignment exchange.
#[derive(Debug)]
pub struct SpawnerClock {
    exchange: Arc<Exchange>,
}

impl SpawnerClock {
    /// Answers the spawned thread: waits for it to run without blocking — on
    /// wasm32 a Worker does not start until the thread that created it
    /// returns to its event loop — then blocks for the exchange itself, which
    /// takes one round trip, for at most `patience`.
    ///
    /// The wait is not bounded — a thread that never runs is a failure its
    /// spawner meets wherever it next waits for that thread — but a spawned
    /// thread that ends or drops its end ends it.
    pub fn answer(self, patience: Duration) -> AnswerWhenReady {
        AnswerWhenReady {
            spawner: Some(self),
            patience,
        }
    }

    /// Answers the spawned thread, blocking: waits for it to run and for the
    /// exchange, for at most `patience` in all. `None` when the spawned thread
    /// did not run in time or went away; it then runs on its own clock.
    ///
    /// Test-only: a Worker does not start until the thread that created it
    /// returns to its event loop, so a spawner that blocks before the thread
    /// runs waits out its patience on wasm32.
    #[cfg(test)]
    fn answer_blocking(self, patience: Duration) -> Option<Alignment> {
        let deadline = Instant::now().checked_add(patience);
        let (state, ready) = self
            .exchange
            .wait_for(self.exchange.lock(), deadline, |step| {
                !matches!(step, Step::Spawned)
            });
        self.exchange_from(state, ready, deadline)
    }

    /// The exchange itself, from a spawned thread that is waiting for it.
    fn exchange_from(
        &self,
        state: MutexGuard<'_, ExchangeState>,
        ready: bool,
        deadline: Option<Instant>,
    ) -> Option<Alignment> {
        let exchange = &self.exchange;
        if !ready || !matches!(state.step, Step::Ready) {
            exchange.advance(state, Step::Abandoned);
            return None;
        }
        exchange.advance(state, Step::Listening);
        let (state, requested) = exchange.wait_for(exchange.lock(), deadline, |step| {
            matches!(step, Step::Requested | Step::Abandoned)
        });
        if !requested || !matches!(state.step, Step::Requested) {
            exchange.advance(state, Step::Abandoned);
            return None;
        }
        // Aligned, so a thread this one spawns is aligned to the clock this one is.
        let answered = ClockInstant::now().0;
        exchange.advance(state, Step::Answered(answered));
        let (state, _) = exchange.wait_for(exchange.lock(), deadline, |step| {
            matches!(step, Step::Aligned(_) | Step::Abandoned)
        });
        match state.step {
            Step::Aligned(alignment) => Some(alignment),
            _ => None,
        }
    }
}

impl Drop for SpawnerClock {
    /// A spawner that goes away before answering releases the spawned thread
    /// to run unaligned.
    fn drop(&mut self) {
        let state = self.exchange.lock();
        if matches!(
            state.step,
            Step::Spawned | Step::Ready | Step::Listening | Step::Requested
        ) {
            self.exchange.advance(state, Step::Abandoned);
        }
    }
}

/// [`SpawnerClock::answer`]: ready once the spawned thread has run and the
/// exchange is over.
#[derive(Debug)]
#[must_use = "a spawner that never polls this never answers"]
pub struct AnswerWhenReady {
    spawner: Option<SpawnerClock>,
    patience: Duration,
}

impl Future for AnswerWhenReady {
    type Output = Option<Alignment>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(spawner) = this.spawner.as_ref() else {
            return Poll::Ready(None);
        };
        let mut state = spawner.exchange.lock();
        if matches!(state.step, Step::Spawned) {
            if state
                .waker
                .as_ref()
                .is_none_or(|waker| !waker.will_wake(context.waker()))
            {
                state.waker = Some(context.waker().clone());
            }
            return Poll::Pending;
        }
        state.waker = None;
        let deadline = Instant::now().checked_add(this.patience);
        let alignment = spawner.exchange_from(state, true, deadline);
        this.spawner = None;
        Poll::Ready(alignment)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Wake;
    use std::thread;

    use super::*;

    const PATIENCE: Duration = Duration::from_secs(10);

    /// Aligns the calling thread and reads back what it recorded.
    fn align(aligning: SpawnedClock) -> Option<Alignment> {
        aligning.align_to_spawner();
        thread_alignment()
    }

    #[test]
    fn arithmetic_is_the_platform_instant_s() {
        let base = ClockInstant::now();
        let later = base + Duration::from_millis(5);
        assert_eq!(later - base, Duration::from_millis(5));
        assert_eq!(base - later, Duration::ZERO, "subtraction saturates");
        assert_eq!(later.duration_since(base), Duration::from_millis(5));
        assert_eq!(base.duration_since(later), Duration::ZERO);
        assert_eq!(base.saturating_duration_since(later), Duration::ZERO);
        assert_eq!(
            later.checked_duration_since(base),
            Some(Duration::from_millis(5))
        );
        assert_eq!(base.checked_duration_since(later), None);
        assert_eq!(base.checked_add(Duration::from_millis(5)), Some(later));
        assert_eq!(later.checked_sub(Duration::from_millis(5)), Some(base));
        assert_eq!(later - Duration::from_millis(5), base);
        let mut moved = base;
        moved += Duration::from_millis(5);
        assert_eq!(moved, later);
        moved -= Duration::from_millis(5);
        assert_eq!(moved, base);
        assert_eq!(base.checked_add(Duration::MAX), None);
        assert!(later.elapsed() <= base.elapsed());
        assert_eq!((base + Duration::from_hours(1)).elapsed(), Duration::ZERO);
    }

    #[test]
    fn instants_order_by_time() {
        let base = ClockInstant::now();
        let mut instants = [
            base + Duration::from_nanos(2),
            base,
            base + Duration::from_nanos(1),
        ];
        instants.sort();
        assert_eq!(
            instants,
            [
                base,
                base + Duration::from_nanos(1),
                base + Duration::from_nanos(2)
            ]
        );
        assert!(ClockInstant::now() >= base);
    }

    #[test]
    fn the_estimate_is_the_answer_less_the_midpoint() {
        let base = Instant::now();
        let at = |millis: u64| base + Duration::from_millis(millis);
        // Asked at 1000, answered with 5000, heard back at 1004.
        assert_eq!(
            estimate(at(1000), at(5000), at(1004)),
            Alignment {
                offset_nanos: 3_998_000_000,
                round_trip: Duration::from_millis(4),
            }
        );
        // The spawner behind: asked at 3000, answered with 1000, back at 3010.
        assert_eq!(
            estimate(at(3000), at(1000), at(3010)),
            Alignment {
                offset_nanos: -2_005_000_000,
                round_trip: Duration::from_millis(10),
            }
        );
        // The same clock: the answer is the midpoint.
        assert_eq!(estimate(at(7), at(8), at(9)).offset_nanos(), 0);
    }

    #[test]
    fn an_offset_moves_the_reading_and_holds_at_the_bounds() {
        let raw = Instant::now();
        assert_eq!(apply_offset(raw, 0), raw);
        assert_eq!(apply_offset(raw, 1_500), raw + Duration::from_nanos(1_500));
        assert_eq!(
            apply_offset(raw, -1_500),
            raw.checked_sub(Duration::from_nanos(1_500)).unwrap()
        );

        let latest = saturate(raw, Duration::MAX, true);
        assert!(latest > raw);
        assert_eq!(latest.checked_add(Duration::from_nanos(1)), None);
        assert_eq!(apply_offset(latest, i64::MAX), latest);
        assert_eq!(apply_offset(latest, 1), latest);
        assert!(apply_offset(latest, -1) < latest);

        let earliest = saturate(raw, Duration::MAX, false);
        assert!(earliest < raw);
        assert_eq!(earliest.checked_sub(Duration::from_nanos(1)), None);
        assert_eq!(apply_offset(earliest, i64::MIN), earliest);
        assert_eq!(apply_offset(earliest, -1), earliest);
        assert!(apply_offset(earliest, 1) > earliest);
    }

    #[test]
    fn a_spawned_thread_aligns_to_its_blocking_spawner() {
        let (answering, aligning) = alignment_channel();
        let child = thread::spawn(move || align(aligning));
        let answered = answering
            .answer_blocking(PATIENCE)
            .expect("the thread aligns");
        assert_eq!(child.join().unwrap(), Some(answered));
        // One process clock natively: the offset is the estimate's error.
        let bound = i64::try_from(answered.round_trip().as_nanos()).unwrap() / 2 + 1;
        assert!(answered.offset_nanos().abs() <= bound, "{answered:?}");
        assert_eq!(
            thread_alignment(),
            None,
            "the spawner itself is not aligned"
        );
    }

    struct Flag(AtomicBool, thread::Thread);

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Release);
            self.1.unpark();
        }
    }

    #[test]
    fn a_spawner_that_must_not_block_waits_for_the_thread_without_blocking() {
        let (answering, aligning) = alignment_channel();
        let mut answer = answering.answer(PATIENCE);
        let flag = Arc::new(Flag(AtomicBool::new(false), thread::current()));
        let waker = Waker::from(Arc::clone(&flag));
        let mut context = Context::from_waker(&waker);
        assert!(
            Pin::new(&mut answer).poll(&mut context).is_pending(),
            "nothing is running yet"
        );
        let child = thread::spawn(move || align(aligning));
        while !flag.0.load(Ordering::Acquire) {
            thread::park_timeout(Duration::from_millis(10));
        }
        let Poll::Ready(answered) = Pin::new(&mut answer).poll(&mut context) else {
            panic!("the thread is ready once it has woken the spawner");
        };
        assert_eq!(child.join().unwrap(), answered);
        assert!(answered.is_some());
    }

    #[test]
    fn a_spawned_thread_that_never_asks_releases_its_spawner() {
        let (answering, aligning) = alignment_channel();
        thread::spawn(move || drop(aligning)).join().unwrap();
        assert_eq!(answering.answer_blocking(PATIENCE), None);
    }

    #[test]
    fn a_spawner_that_gives_up_releases_the_thread_unaligned() {
        let (answering, aligning) = alignment_channel();
        assert_eq!(
            answering.answer_blocking(Duration::from_millis(1)),
            None,
            "nothing ran within the patience"
        );
        let child = thread::spawn(move || align(aligning));
        assert_eq!(child.join().unwrap(), None);
    }

    #[test]
    fn a_dropped_spawner_releases_a_waiting_thread() {
        let (answering, aligning) = alignment_channel();
        let child = thread::spawn(move || align(aligning));
        // Whether the thread is already waiting or not yet started, a
        // dropped spawner ends its wait.
        drop(answering);
        assert_eq!(child.join().unwrap(), None);
    }

    #[test]
    fn a_thread_spawned_by_an_aligned_thread_aligns_to_the_same_clock() {
        let (answering, aligning) = alignment_channel();
        let middle = thread::spawn(move || {
            let first = align(aligning);
            let (answering, aligning) = alignment_channel();
            let last = thread::spawn(move || align(aligning));
            let second = answering.answer_blocking(PATIENCE);
            (first, second, last.join().unwrap())
        });
        let outer = answering.answer_blocking(PATIENCE);
        let (first, second, last) = middle.join().unwrap();
        assert!(outer.is_some());
        assert_eq!(first, outer);
        assert_eq!(second, last);
        assert!(last.is_some());
    }
}
