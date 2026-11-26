//! What a view on `bobcat-main` and a worker on `bobcat-workers` have in
//! common: the tasks that act on one realm-owning object, the one signal that
//! ends them, the owner's wait, and the one task that waits out that realm's
//! clock.
//!
//! Both used to carry a copy of this. Each held a `Vec<JoinHandle>` it pruned
//! on every spawn, a private `mpsc` the end reason travelled on, an unwind
//! guard, and a reap that aborted the handles, awaited them in order and
//! collected whichever panic it found first — two spellings of one mechanism,
//! differing only in what they were named after. What is left of that here is
//! one [`Lifetime`]: a [`JoinSet`] the owner consumes continuously, a
//! [`CancellationToken`] every thread can read, the latch that says whether
//! *this* thread has ended the object, the two latches that keep the report
//! that ended it and the report of a panic to one each, the two numbers its
//! clock task reads — the deadline this realm armed and the generation its
//! own last entry ran the shared job queue up to — and the handle on the
//! engine thread its tasks are spawned onto and its entries are queued on.
//!
//! # Tasks and entries
//!
//! An *entry* into the object's realm is a job on the engine thread
//! [`crate::jobs`] describes, queued by whichever task decided one was owed and
//! awaited by it: [`run_job`] is the one way one is queued, and
//! [`Settles::settle`] the epilogue-only entry a wake that carries no operation
//! runs. What an entry, an end and a release do with the realm is
//! [`crate::realm::owner`]'s, written over this once for both kinds of owner.
//!
//! # The token
//!
//! One signal instead of the three that used to be synchronised by hand — a
//! flag the embedder set, a `Cell` the engine thread read, and a channel that
//! woke the owner. It answers all three: `is_cancelled()` is the synchronous
//! question a host asks through a `SourceCompletion`, `cancelled()` is the
//! owner's wait, and `cancel()` is what a release, a fatal event, an exit
//! guard or the engine's own end does.
//!
//! A view and each Worker have independent tokens. Host release ends the
//! view's ordinary tasks; MTS then finishes its JS disposal RPC before its
//! realm is released. A Worker ends on explicit termination, collection of
//! its MTS handle, its own `close()`, or its message channel closing.
//!
//! # Why the latch stays
//!
//! `token.is_cancelled()` is a mutex read, and it can flip between two
//! statements of one synchronous entry, because another thread is what writes
//! it. So it is never what an entry checks. [`Lifetime::ended`] is a plain
//! `Cell` written only on this thread: it costs a load, and nothing but this
//! thread writes it.
//!
//! It is stable for the length of an entry with one exception, which is the
//! point of [`crate::jobs`]: a job that parks on a synchronous wait lets this
//! thread's *tasks* run, and one of them may call [`Lifetime::end`]. So the
//! latch can flip at a wait point inside an entry, and only there. The
//! epilogue that follows the wait sees the end and does nothing, exactly as it
//! already does for an operation that reported a failure of its own. `end`,
//! `terminal` and their callers touch no realm borrow, which is what keeps them
//! callable from a task at any time.
//!
//! # What ended the object
//!
//! Nothing records it. Both reason enums are gone, because nothing read them:
//! what the embedder was told is what was *reported* before the end — a
//! view's `StartupFailed`, a worker's `Failed` or `Closed`, the first of which
//! is the only one [`Lifetime::report_terminal`] lets through — and what it
//! was not told is a release, which is the token having been cancelled from
//! outside. Any other script failure is not an end at all: it is reported and
//! the object goes on. A panic is the one end that still owes a report — a
//! view's is `Panicked` — and the payload is always available for it: the set
//! holds a finished task until the owner joins it, so the `JoinError` reaches
//! the owner rather than being pruned away before it could.
//!
//! # What is not here
//!
//! The two engine threads each run a `JoinSet` of their own — `group_task`'s
//! views and `serve_workers`'s workers, each with a side map naming who to
//! report a trapped task to. Those are the *thread's* tasks rather than one
//! realm's: nothing about them has an end signal, an entry boundary or an
//! epilogue, so they stay where they are. What such a report sends is still
//! the realm kind's: the event is built by the Panic row of its table in
//! [`crate::realm::policy`], the row an owner's own panic is reported through.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::rc::Rc;

use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::clock::ClockInstant;
use crate::jobs::{JobKind, JsThreadHandle, JsThreadName};
use crate::link::HostOutbox;
use crate::performance::{EntryTypes, LongTaskEntry, PerformanceEntry, Timestamp, WorkerAudience};

pub(crate) struct Lifetime {
    tasks: RefCell<JoinSet<()>>,
    token: CancellationToken,
    ended: Cell<bool>,
    terminal_reported: Cell<bool>,
    panic_reported: Cell<bool>,
    deadline: watch::Sender<Option<ClockInstant>>,
    own_checkpoint: Cell<u64>,
    thread: JsThreadHandle,
    #[cfg(test)]
    epilogues: Cell<u64>,
}

impl Lifetime {
    pub(crate) fn new(token: CancellationToken, thread: JsThreadHandle) -> Self {
        Self {
            tasks: RefCell::new(JoinSet::new()),
            token,
            ended: Cell::new(false),
            terminal_reported: Cell::new(false),
            panic_reported: Cell::new(false),
            deadline: watch::channel(None).0,
            own_checkpoint: Cell::new(0),
            thread,
            #[cfg(test)]
            epilogues: Cell::new(0),
        }
    }

    pub(crate) const fn thread(&self) -> &JsThreadHandle {
        &self.thread
    }

    pub(crate) fn spawn(&self, future: impl Future<Output = ()> + 'static) {
        self.thread.spawn_into(&mut self.tasks.borrow_mut(), future);
    }

    pub(crate) fn end(&self) -> bool {
        if self.ended.replace(true) {
            return false;
        }
        self.token.cancel();
        self.deadline
            .send_if_modified(|deadline| deadline.take().is_some());
        true
    }

    pub(crate) fn ended(&self) -> bool {
        self.ended.get()
    }

    pub(crate) fn arm_deadline(&self, deadline: Option<ClockInstant>) {
        self.deadline.send_if_modified(|armed| {
            if *armed == deadline {
                return false;
            }
            *armed = deadline;
            true
        });
    }

    pub(crate) fn record_checkpoint(&self, generation: u64) {
        self.own_checkpoint.set(generation);
    }

    pub(crate) fn deadlines(&self) -> watch::Receiver<Option<ClockInstant>> {
        self.deadline.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn armed_deadline(&self) -> Option<ClockInstant> {
        *self.deadline.borrow()
    }

    pub(crate) const fn token(&self) -> &CancellationToken {
        &self.token
    }

    pub(crate) fn report_terminal(&self) -> bool {
        !self.terminal_reported.replace(true)
    }

    pub(crate) fn report_panic(&self) -> bool {
        !self.panic_reported.replace(true)
    }

    #[cfg(test)]
    pub(crate) fn count_epilogue(&self) {
        self.epilogues.set(self.epilogues.get() + 1);
    }

    #[cfg(test)]
    pub(crate) fn epilogue_count(&self) -> u64 {
        self.epilogues.get()
    }

    pub(crate) async fn serve(&self, on_panic: &mut dyn FnMut(Box<dyn Any + Send>)) {
        loop {
            let finished = tokio::select! {
                () = self.token.cancelled() => return,
                finished = self.join_next() => finished,
            };
            let Some(finished) = finished else { return };
            report(finished, on_panic);
        }
    }

    pub(crate) async fn reap(&self, on_panic: &mut dyn FnMut(Box<dyn Any + Send>)) {
        self.tasks.borrow_mut().abort_all();
        while let Some(finished) = self.join_next().await {
            report(finished, on_panic);
        }
    }

    fn join_next(&self) -> impl Future<Output = Option<Result<(), JoinError>>> {
        poll_fn(|context| self.tasks.borrow_mut().poll_join_next(context))
    }

    #[cfg(test)]
    pub(crate) fn task_count(&self) -> usize {
        self.tasks.borrow().len()
    }
}

pub(crate) trait Settles: Sized + 'static {
    fn lifetime(&self) -> &Lifetime;

    const THREAD: JsThreadName;

    fn outbox(&self) -> &HostOutbox;

    fn settle(owner: &Rc<Self>, kind: JobKind) -> impl Future<Output = Option<()>>;

    fn end(owner: &Rc<Self>);

    fn trapped(owner: &Rc<Self>, payload: &(dyn Any + Send));
}

pub(crate) fn run_job<S, T, B>(
    owner: &Rc<S>,
    kind: JobKind,
    body: B,
) -> impl Future<Output = Option<T>> + use<S, T, B>
where
    S: Settles,
    T: 'static,
    B: FnOnce(&Rc<S>) -> Option<T> + 'static,
{
    let thread = owner.lifetime().thread().clone();
    let owner = Rc::clone(owner);
    let answer = thread.run(move || {
        let registry = owner.outbox().registry();
        let timed = registry
            .long_task_threshold()
            .map(|threshold| (threshold, ClockInstant::now()));
        let value = match catch_unwind(AssertUnwindSafe(|| body(&owner))) {
            Ok(value) => value,
            Err(payload) => {
                S::trapped(&owner, payload.as_ref());
                None
            }
        };
        if let Some((threshold, start)) = timed {
            let end = ClockInstant::now();
            if end.saturating_duration_since(start) >= threshold
                && registry.deliver_entry(
                    PerformanceEntry::LongTask(LongTaskEntry {
                        start: Timestamp(start),
                        end: Timestamp(end),
                        thread: S::THREAD,
                        kind,
                    }),
                    EntryTypes::LONG_TASK,
                    WorkerAudience::None,
                )
            {
                owner.outbox().wake();
            }
        }
        value
    });
    async move { answer.await.flatten() }
}

pub(crate) async fn serve_clock<S: Settles>(
    owner: Rc<S>,
    mut deadlines: watch::Receiver<Option<ClockInstant>>,
    mut checkpoints: watch::Receiver<u64>,
) {
    let mut armed: Option<ClockInstant> = None;
    let mut sleep = pin!(crate::clock::sleep_until(ClockInstant::now()));
    loop {
        if owner.lifetime().ended() {
            return;
        }
        let deadline = *deadlines.borrow_and_update();
        if deadline != armed {
            if let Some(deadline) = deadline {
                sleep.set(crate::clock::sleep_until(deadline));
            }
            armed = deadline;
        }
        tokio::select! {
            () = &mut sleep, if armed.is_some() => {
                armed = None;
                let _ = S::settle(&owner, JobKind::Timer).await;
            }
            changed = deadlines.changed() => if changed.is_err() { return },
            changed = checkpoints.changed() => {
                if changed.is_err() {
                    return;
                }
                if *checkpoints.borrow_and_update() == owner.lifetime().own_checkpoint.get() {
                    continue;
                }
                let _ = S::settle(&owner, JobKind::Settle).await;
            }
        }
    }
}

pub(crate) struct EndOnUnwind<S: Settles>(Rc<S>);

impl<S: Settles> EndOnUnwind<S> {
    pub(crate) fn new(owner: &Rc<S>) -> Self {
        Self(Rc::clone(owner))
    }
}

impl<S: Settles> Drop for EndOnUnwind<S> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            S::end(&self.0);
        }
    }
}

fn report(finished: Result<(), JoinError>, on_panic: &mut dyn FnMut(Box<dyn Any + Send>)) {
    let Err(error) = finished else { return };
    if !error.is_panic() {
        return;
    }
    on_panic(error.into_panic());
}
