//! What a view on `bobcat-main` and a worker on `bobcat-workers` have in
//! common: the tasks that act on one realm-owning object, the one signal that
//! ends them, and the owner's wait.
//!
//! Both used to carry a copy of this. Each held a `Vec<JoinHandle>` it pruned
//! on every spawn, a private `mpsc` the end reason travelled on, an unwind
//! guard, and a reap that aborted the handles, awaited them in order and
//! collected whichever panic it found first — two spellings of one mechanism,
//! differing only in what they were named after. What is left of that here is
//! one [`Lifetime`]: a [`JoinSet`] the owner consumes continuously, a
//! [`CancellationToken`] every thread can read, and the latch that says whether
//! *this* thread has ended the object.
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
//! Its parent/child relation is the ownership: the embedder holds a view's
//! token and cancels it when the view is dropped, and every worker that view's
//! realm creates carries a child of it, so releasing a view ends the workers it
//! created without a message having to reach each of them first.
//!
//! # Why the latch stays
//!
//! `token.is_cancelled()` is a mutex read, and it can flip between two
//! statements of one synchronous entry, because another thread is what writes
//! it. So it is never what an entry checks. [`Lifetime::ended`] is a plain
//! `Cell` written only on this thread: it is stable for the length of an entry
//! and costs a load. Cancellation arriving from elsewhere does not touch it —
//! the owner mirrors it by calling [`Lifetime::end`] itself when it wakes,
//! which is the window a task has to run one more entry after a release.
//!
//! # What ended the object
//!
//! Nothing records it. Both reason enums are gone, because nothing read them:
//! what the embedder was told is what was *reported* before the end — a
//! `StartupFailed`, a `ScriptRunError`, a worker's `Failed` or `Closed` — and
//! what it was not told is a release, which is the token having been cancelled
//! from outside. A panic is the one end that still owes a report, and the
//! payload is always available for it: the set holds a finished task until the
//! owner joins it, so the `JoinError` reaches the owner rather than being
//! pruned away before it could.
//!
//! # What is not here
//!
//! The two engine threads each run a `JoinSet` of their own — `group_task`'s
//! views and `serve_workers`'s workers, each with a side map naming who to
//! report a trapped task to. Those are the *thread's* tasks rather than one
//! realm's: nothing about them has an end signal, an entry boundary or an
//! epilogue, so they stay where they are.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::future::{Future, poll_fn};
use std::rc::Rc;

use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

/// The tasks that act on one realm-owning object, the one signal that ends
/// them, and the latch that says whether this thread has ended it.
///
/// `!Sync` — the `Cell`s and the `RefCell`. It is only ever reached through an
/// `Rc` inside a `!Send` owner on the `LocalSet`'s thread, which is what keeps
/// it with the realm its tasks enter.
pub(crate) struct Lifetime {
    /// Every task of the object. Borrowed only inside synchronous calls and
    /// inside single polls, never across an `.await`, which is why a task can
    /// spawn a sibling while the owner is parked on the set.
    tasks: RefCell<JoinSet<()>>,
    /// The end signal every thread can read and cancel.
    token: CancellationToken,
    /// The end as this thread knows it. See the module doc: this is what an
    /// entry checks, and the token is not.
    ended: Cell<bool>,
    /// The latch the one payload-bearing panic report goes through.
    ///
    /// Separate from the owner's own report latch, which a `StartupFailed` or
    /// a worker's `Failed` has already spent by the time a task traps: a panic
    /// is reported whatever else was reported before it, and exactly once.
    /// [`Self::serve`], [`Self::reap`] and a panic the owner caught inside its
    /// own entry boundary all share this one.
    panic_reported: Cell<bool>,
}

impl Lifetime {
    pub(crate) fn new(token: CancellationToken) -> Rc<Self> {
        Rc::new(Self {
            tasks: RefCell::new(JoinSet::new()),
            token,
            ended: Cell::new(false),
            panic_reported: Cell::new(false),
        })
    }

    /// Starts one more task of this object.
    ///
    /// The future is taken as it is. What a panic in it costs the object is
    /// the *owner's* business — a `BeginFrame` to acknowledge, a deadline to
    /// withdraw — so the guard that ends the object during the unwind is the
    /// owner's wrapper around this rather than something stored here, which
    /// would be an `Rc` cycle through the object that holds this.
    pub(crate) fn spawn(&self, future: impl Future<Output = ()> + 'static) {
        self.tasks.borrow_mut().spawn_local(future);
    }

    /// Ends the object, once. `true` for the call that did it.
    ///
    /// Synchronous, because the latch is what makes an end immediate: every
    /// entry point returns at once when it is set, and the owner's abort only
    /// reclaims the tasks rather than being what stops them.
    pub(crate) fn end(&self) -> bool {
        if self.ended.replace(true) {
            return false;
        }
        self.token.cancel();
        true
    }

    pub(crate) fn ended(&self) -> bool {
        self.ended.get()
    }

    pub(crate) const fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Whether this is the first panic report of the object's life. `true`
    /// exactly once, for whichever of the owner's paths saw the panic first.
    pub(crate) fn report_panic(&self) -> bool {
        !self.panic_reported.replace(true)
    }

    /// The owner's one wait: the end, or the next task to finish.
    ///
    /// Returns when the token is cancelled — by this thread's own [`Self::end`]
    /// or by an embedder releasing what owns this — or when the set is empty,
    /// which is every task of the object having returned. A task that returned
    /// normally is nothing to answer; a task that panicked hands its payload to
    /// `on_panic`; an aborted task is ignored, since the abort is the owner's
    /// own doing.
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

    /// Aborts every task of the object and drains the set.
    ///
    /// The abort is what makes a parked task return; the latch is what already
    /// stopped it from doing anything. Awaiting each of them is what makes the
    /// owner the last holder of the object: a task holds an `Rc` of it until
    /// its future is dropped.
    pub(crate) async fn reap(&self, on_panic: &mut dyn FnMut(Box<dyn Any + Send>)) {
        self.tasks.borrow_mut().abort_all();
        while let Some(finished) = self.join_next().await {
            report(finished, on_panic);
        }
    }

    /// The next task of this object to finish, `None` for a set that is empty.
    ///
    /// Hand-polled rather than `JoinSet::join_next`, because that takes the set
    /// by `&mut` for the whole of an `.await` while a task of this object may
    /// be spawning a sibling into it. Here the `RefMut` lives inside one poll:
    /// polling a `JoinHandle` runs no task, so nothing of this object's can be
    /// entered while it is held.
    fn join_next(&self) -> impl Future<Output = Option<Result<(), JoinError>>> {
        poll_fn(|context| self.tasks.borrow_mut().poll_join_next(context))
    }

    /// How many tasks the set still holds, finished-and-unjoined included.
    #[cfg(test)]
    pub(crate) fn task_count(&self) -> usize {
        self.tasks.borrow().len()
    }
}

/// Hands a panic's payload on, once the borrow of the set that yielded it has
/// been released — `on_panic` is the owner's, and reports and ends the object.
fn report(finished: Result<(), JoinError>, on_panic: &mut dyn FnMut(Box<dyn Any + Send>)) {
    let Err(error) = finished else { return };
    if !error.is_panic() {
        return;
    }
    on_panic(error.into_panic());
}
