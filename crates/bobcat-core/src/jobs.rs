//! What an engine thread is: a tokio scheduler, and a queue of JavaScript
//! jobs the thread runs between two turns of it.
//!
//! Both `bobcat-main` and `bobcat-workers` are one [`JsThread`]. It owns a
//! `current_thread` tokio runtime, the `LocalSet` every task of every view or
//! worker on that thread lives on, and a FIFO of jobs.
//!
//! # The two halves
//!
//! **Tasks wait and route.** A task reads a channel, waits out a deadline,
//! mirrors a cancellation, spawns a sibling. It never touches a realm, a
//! document or the shared `ScriptRuntime`; what it does with what it read is
//! push a job.
//!
//! **Jobs are the only place JavaScript runs.** [`JsThread::run`]'s loop pops
//! them one at a time and calls them *outside* the scheduler — outside any
//! `block_on`, outside any task's `poll`. That is the whole point of the
//! split: a job may block.
//!
//! # A synchronous wait
//!
//! [`JsThread::wait`] is what a host member that must answer synchronously —
//! `waitFuture` and `adoptStyleSheet` today — parks on. It is a fresh
//! `block_on` over the same `LocalSet`, which is legal precisely because a job
//! runs outside one. While it waits:
//!
//! - every task on this thread keeps running: channel reads, lifecycle signals, acknowledgements,
//!   resource routing, the timers of other realms;
//! - **no other job runs.** Jobs pushed meanwhile queue behind the waiting one and run, in order,
//!   after it returns. So a job may hold a `RefMut` of its realm and of the shared runtime across
//!   its own wait, and no JavaScript of a sibling view interleaves with it.
//!
//! A job is therefore the unit of "JavaScript is running", and a wait inside
//! one blocks JavaScript alone.
//!
//! # Ownership
//!
//! The top loop holds the only strong `Rc<JsThread>`. Everything reachable
//! from a task or a realm — a `GroupContext`, a `Page`, a `Worker`, a
//! `Lifetime`, a host closure — holds a [`JsThreadHandle`], which is a `Weak`:
//! the thread owns the `LocalSet`, the `LocalSet` owns the tasks, and a task
//! owns the objects whose host closures would otherwise point back here.
//! Pushing onto a thread that is gone drops the job, and the future that was
//! waiting for its answer resolves to `None`.
//!
//! # The tokio trap
//!
//! The free `tokio::task::spawn_local` and `JoinSet::spawn_local` panic when
//! called outside a `LocalSet` context, and a job *is* outside one. Every
//! spawn a job can reach therefore goes through [`JsThreadHandle::spawn_into`],
//! which names the set explicitly. For the same reason a timer future is built
//! inside an async block rather than in job context.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::rc::{Rc, Weak};

use tokio::sync::{Notify, oneshot};
use tokio::task::{JoinSet, LocalSet};

/// One unit of JavaScript-touching work, as the queue holds it.
type Job = Box<dyn FnOnce()>;

/// One engine thread: its scheduler, and the jobs it runs between two turns of
/// it.
pub(crate) struct JsThread {
    /// **Field order is the release order.** The queue goes first, so a job
    /// that was never run — and the `Rc`s it captured — is dropped while the
    /// `LocalSet` and the runtime it may name are still standing; the
    /// `LocalSet` next, which drops every task; the runtime last.
    queue: RefCell<VecDeque<Job>>,
    /// What brings the top loop back out of `block_on` to run a job.
    ///
    /// The case it exists for is the ordinary one: a task pushes a job while
    /// the loop is parked on `notified()`, and `notify_one` wakes it there and
    /// then. A push that lands while the loop is *not* polling `notified()` —
    /// during [`JsThread::drain`], or inside a job's own
    /// [`wait`](JsThread::wait) — leaves the stored permit behind instead, and
    /// costs at most one extra turn of the loop rather than a lost wake: the
    /// job itself is already in the queue, and `drain` runs the queue to
    /// exhaustion before every park.
    pushed: Notify,
    local: LocalSet,
    runtime: tokio::runtime::Runtime,
}

impl JsThread {
    pub(crate) fn new() -> Rc<Self> {
        let mut builder = tokio::runtime::Builder::new_current_thread();
        // Natively the engine waits out its own realms' timers on tokio's
        // timer. On wasm32 that timer reads `std::time::Instant`, which panics
        // there, so `crate::clock::sleep_until` serves the same waits through a
        // thread of this crate's own instead.
        #[cfg(not(target_arch = "wasm32"))]
        builder.enable_time();
        Rc::new(Self {
            queue: RefCell::new(VecDeque::new()),
            pushed: Notify::new(),
            local: LocalSet::new(),
            runtime: builder
                .build()
                .expect("a current-thread runtime asks the platform for nothing"),
        })
    }

    /// A weak handle for everything that lives inside a task or a realm.
    pub(crate) fn handle(self: &Rc<Self>) -> JsThreadHandle {
        JsThreadHandle(Rc::downgrade(self))
    }

    /// This thread's whole body: run `main` as a task, and run the jobs its
    /// tasks push.
    ///
    /// `main` is a task rather than the loop itself so that it keeps running
    /// during a job's synchronous wait — a view must be able to attach, and a
    /// finished one to be joined, while a sibling is parked on a stylesheet.
    ///
    /// Returning is `main` having finished, and what is still queued when it
    /// does is dropped unrun: `main` is the whole of this thread's work — the
    /// group's views, or the thread's workers — so a job left behind it has
    /// nothing left to act on. A panic in `main` is resumed here, so a trap in
    /// `group_task` or `serve_workers` still unwinds the thread.
    pub(crate) fn run(self: &Rc<Self>, main: impl Future<Output = ()> + 'static) {
        let mut main = self.local.spawn_local(main);
        let finished = loop {
            // Outside `block_on`, which is what makes a job's own `wait`
            // legal: it is not a runtime started from within a runtime.
            self.drain();
            if let Some(result) = self.runtime.block_on(self.local.run_until(async {
                tokio::select! {
                    biased;
                    result = &mut main => Some(result),
                    () = self.pushed.notified() => None,
                }
            })) {
                break result;
            }
        };
        // Dropped here rather than with the struct so the `Rc`s a job captured
        // — a page, a worker, and through them this thread's one sender to
        // `bobcat-workers` — are released before the `LocalSet` is.
        self.queue.borrow_mut().clear();
        if let Err(error) = finished
            && error.is_panic()
        {
            std::panic::resume_unwind(error.into_panic());
        }
    }

    /// Runs every queued job, in order, until the queue is empty. A job that
    /// pushes another is followed by it.
    fn drain(&self) {
        loop {
            let Some(job) = self.queue.borrow_mut().pop_front() else {
                return;
            };
            job();
        }
    }

    /// Blocks the job that is running until `future` is ready, driving this
    /// thread's tasks meanwhile and none of its jobs.
    ///
    /// **Only legal from job context.** From inside a task's `poll` this is a
    /// runtime started from within a runtime, and tokio says so.
    fn wait<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(self.local.run_until(future))
    }
}

/// How everything that lives inside a task or a realm reaches its thread.
///
/// A `Weak`, because the loop owns the `LocalSet`, the `LocalSet` owns the
/// tasks, and a task owns the page or worker whose host closures hold this.
#[derive(Clone)]
pub(crate) struct JsThreadHandle(Weak<JsThread>);

impl JsThreadHandle {
    /// Queues one job and answers with what it returned.
    ///
    /// `None` is a job that never ran: the thread was already gone, or the
    /// loop cleared the queue before reaching it.
    ///
    /// The job is queued by this call rather than by the future it returns,
    /// which is why this is not an `async fn`: dropping that future must not
    /// un-queue the job. A caller with nothing to wait for — the deliveries in
    /// `consume_messages`, which must go on reading — drops it and the job
    /// still runs.
    pub(crate) fn run<T, J>(&self, job: J) -> impl Future<Output = Option<T>> + use<T, J>
    where
        T: 'static,
        J: FnOnce() -> T + 'static,
    {
        let (done, answer) = oneshot::channel();
        self.push(Box::new(move || {
            let _ = done.send(job());
        }));
        async move { answer.await.ok() }
    }

    /// Starts one task on this thread's `LocalSet`, naming the set rather than
    /// reading it out of the ambient context — which a job does not have.
    pub(crate) fn spawn_into(
        &self,
        tasks: &mut JoinSet<()>,
        future: impl Future<Output = ()> + 'static,
    ) {
        let thread = self
            .0
            .upgrade()
            .expect("a task is only ever spawned while its engine thread is running");
        tasks.spawn_local_on(future, &thread.local);
    }

    /// The synchronous wait a host member parks on. See [`JsThread::wait`].
    pub(crate) fn wait<F: Future>(&self, future: F) -> F::Output {
        let thread = self
            .0
            .upgrade()
            .expect("a job only runs while its engine thread is running");
        thread.wait(future)
    }

    /// How many jobs are waiting to run, for the tests that pin that a task
    /// parked behind a job does not queue work without bound.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.0
            .upgrade()
            .map_or(0, |thread| thread.queue.borrow().len())
    }

    fn push(&self, job: Job) {
        // A thread that is gone runs nothing more, and the job is dropped
        // here: whoever wanted its answer hears `None`.
        let Some(thread) = self.0.upgrade() else {
            return;
        };
        thread.queue.borrow_mut().push_back(job);
        thread.pushed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// What a test records the order of what ran in.
    type Log = Rc<RefCell<Vec<&'static str>>>;

    #[test]
    fn jobs_run_in_the_order_they_were_pushed() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let log: Log = Rc::default();
        for name in ["first", "second", "third"] {
            let log = Rc::clone(&log);
            drop(handle.run(move || log.borrow_mut().push(name)));
        }
        thread.run(async {});
        assert_eq!(*log.borrow(), ["first", "second", "third"]);
    }

    /// The whole point of the split: a job pushed while another job is inside
    /// a synchronous wait does not run until that job returns, even though the
    /// task that pushed it ran during the wait.
    #[test]
    fn a_job_pushed_during_a_wait_runs_only_after_the_waiting_job_returns() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let log: Log = Rc::default();
        let (release, released) = oneshot::channel::<()>();

        {
            let (handle, log) = (handle.clone(), Rc::clone(&log));
            drop(handle.clone().run(move || {
                log.borrow_mut().push("waiting job entered");
                // The task below runs inside this wait and pushes a job; the
                // answer this waits for is what that same task sends.
                handle.wait(async move {
                    let _ = released.await;
                });
                log.borrow_mut().push("waiting job returned");
            }));
        }

        let pusher = {
            let (handle, log) = (handle.clone(), Rc::clone(&log));
            async move {
                // Spawned onto the set by `run` below, so this runs inside the
                // waiting job's `wait`.
                log.borrow_mut().push("task ran during the wait");
                drop(handle.run({
                    let log = Rc::clone(&log);
                    move || log.borrow_mut().push("job pushed during the wait")
                }));
                let _ = release.send(());
            }
        };
        thread.run(pusher);

        assert_eq!(
            *log.borrow(),
            [
                "waiting job entered",
                "task ran during the wait",
                "waiting job returned",
                "job pushed during the wait",
            ]
        );
    }

    /// A job pushed by a job is run by the same drain, behind the one that
    /// pushed it. That is what makes `main` finishing a clean end: the loop
    /// never has to come back for work a job left, so the only thing dropped
    /// unrun is what a *task* queued and nobody waited for.
    #[test]
    fn a_job_pushed_by_a_job_runs_in_the_same_drain() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let log: Log = Rc::default();
        {
            let (handle, log) = (handle.clone(), Rc::clone(&log));
            drop(handle.clone().run(move || {
                log.borrow_mut().push("first");
                let log = Rc::clone(&log);
                drop(handle.run(move || log.borrow_mut().push("second")));
            }));
        }
        // `main` finishes at once and nothing is drained after it, so the
        // second job runs only because the first drain picked it up.
        thread.run(async {});
        assert_eq!(*log.borrow(), ["first", "second"]);
    }

    /// A job still queued when `main` finishes is dropped unrun. `main` is the
    /// whole of the thread's work, so there is nothing left for a job to act
    /// on; queued *by a job* is the other case, and the test above covers it.
    #[test]
    fn a_job_still_queued_when_main_finishes_is_dropped_unrun() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let log: Log = Rc::default();
        let body = {
            let (handle, log) = (handle.clone(), Rc::clone(&log));
            async move {
                drop(handle.run(move || log.borrow_mut().push("never ran")));
            }
        };
        thread.run(body);
        assert!(log.borrow().is_empty(), "the queued job never ran");
    }

    #[test]
    fn a_job_answers_the_future_the_pusher_kept() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let answered = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&answered);
        thread.run(async move {
            *sink.borrow_mut() = handle.run(|| 7_u32).await;
        });
        assert_eq!(*answered.borrow(), Some(7));
    }

    /// `main` finishing is what ends the loop, whatever else is still parked
    /// on the set.
    #[test]
    fn the_loop_ends_when_main_finishes() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let log: Log = Rc::default();
        // Held outside `main`, so the parked task is still on the set when the
        // loop exits rather than being dropped with the future that spawned it.
        let tasks = Rc::new(RefCell::new(JoinSet::new()));
        let body = {
            let (handle, log, tasks) = (handle.clone(), Rc::clone(&log), Rc::clone(&tasks));
            async move {
                // A task that never returns, spawned the way a `Lifetime`
                // spawns one — by naming the set rather than reading it out of
                // the ambient context.
                handle.spawn_into(&mut tasks.borrow_mut(), std::future::pending());
                log.borrow_mut().push("main ran");
            }
        };
        thread.run(body);
        assert_eq!(*log.borrow(), ["main ran"]);
    }

    /// A task spawned from job context reaches the set, which is the whole
    /// reason `spawn_into` names it: a job has no ambient `LocalSet`.
    #[test]
    fn a_task_spawned_from_job_context_runs() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let log: Log = Rc::default();
        let tasks = Rc::new(RefCell::new(JoinSet::new()));
        let (spawned, ran) = oneshot::channel::<()>();
        {
            let (handle, log, tasks) = (handle.clone(), Rc::clone(&log), Rc::clone(&tasks));
            drop(handle.clone().run(move || {
                handle.spawn_into(&mut tasks.borrow_mut(), async move {
                    log.borrow_mut().push("spawned from a job");
                    let _ = spawned.send(());
                });
            }));
        }
        {
            let handle = handle.clone();
            drop(handle.clone().run(move || {
                // Behind the spawn, so the task above is on the set by now,
                // and this wait is what lets it run.
                handle.wait(async {
                    let _ = ran.await;
                });
            }));
        }
        thread.run(async {});
        assert_eq!(*log.borrow(), ["spawned from a job"]);
    }
}
