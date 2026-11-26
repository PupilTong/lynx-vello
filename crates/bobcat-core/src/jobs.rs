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

type Job = Box<dyn FnOnce()>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsThreadName {
    Main,
    Workers,
}

impl JsThreadName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "Lynx_TASM",
            Self::Workers => "Lynx_JS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JobKind {
    Start,
    Entry,
    Module,
    Future,
    Timer,
    Settle,
    Command,
    ContentVisibility,
    Intersection,
    ResizeObserver,
    ComponentEvent,
    FontFace,
    WorkerEvent,
    Post,
    AnimationFrame,
    ModuleCallback,
    Performance,
    Memory,
    Release,
}

impl JobKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Entry => "entry",
            Self::Module => "module",
            Self::Future => "future",
            Self::Timer => "timer",
            Self::Settle => "settle",
            Self::Command => "command",
            Self::ContentVisibility => "contentVisibility",
            Self::Intersection => "intersection",
            Self::ResizeObserver => "resizeObserver",
            Self::ComponentEvent => "componentEvent",
            Self::FontFace => "fontFace",
            Self::WorkerEvent => "workerEvent",
            Self::Post => "post",
            Self::AnimationFrame => "animationFrame",
            Self::ModuleCallback => "moduleCallback",
            Self::Performance => "performance",
            Self::Memory => "memory",
            Self::Release => "release",
        }
    }
}

pub(crate) struct JsThread {
    queue: RefCell<VecDeque<Job>>,
    pushed: Notify,
    local: LocalSet,
    runtime: tokio::runtime::Runtime,
}

impl JsThread {
    pub(crate) fn new() -> Rc<Self> {
        let mut builder = tokio::runtime::Builder::new_current_thread();
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

    pub(crate) fn handle(self: &Rc<Self>) -> JsThreadHandle {
        JsThreadHandle(Rc::downgrade(self))
    }

    pub(crate) fn run(self: &Rc<Self>, main: impl Future<Output = ()> + 'static) {
        let mut main = self.local.spawn_local(main);
        let finished = loop {
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
        self.queue.borrow_mut().clear();
        if let Err(error) = finished
            && error.is_panic()
        {
            std::panic::resume_unwind(error.into_panic());
        }
    }

    fn drain(&self) {
        loop {
            let Some(job) = self.queue.borrow_mut().pop_front() else {
                return;
            };
            job();
        }
    }

    fn wait<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(self.local.run_until(future))
    }
}

#[derive(Clone)]
pub(crate) struct JsThreadHandle(Weak<JsThread>);

impl JsThreadHandle {
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

    pub(crate) fn wait<F: Future>(&self, future: F) -> F::Output {
        let thread = self
            .0
            .upgrade()
            .expect("a job only runs while its engine thread is running");
        thread.wait(future)
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.0
            .upgrade()
            .map_or(0, |thread| thread.queue.borrow().len())
    }

    fn push(&self, job: Job) {
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
                handle.wait(async move {
                    let _ = released.await;
                });
                log.borrow_mut().push("waiting job returned");
            }));
        }

        let pusher = {
            let (handle, log) = (handle.clone(), Rc::clone(&log));
            async move {
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
        thread.run(async {});
        assert_eq!(*log.borrow(), ["first", "second"]);
    }

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

    #[test]
    fn the_loop_ends_when_main_finishes() {
        let thread = JsThread::new();
        let handle = thread.handle();
        let log: Log = Rc::default();
        let tasks = Rc::new(RefCell::new(JoinSet::new()));
        let body = {
            let (handle, log, tasks) = (handle.clone(), Rc::clone(&log), Rc::clone(&tasks));
            async move {
                handle.spawn_into(&mut tasks.borrow_mut(), std::future::pending());
                log.borrow_mut().push("main ran");
            }
        };
        thread.run(body);
        assert_eq!(*log.borrow(), ["main ran"]);
    }

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
                handle.wait(async {
                    let _ = ran.await;
                });
            }));
        }
        thread.run(async {});
        assert_eq!(*log.borrow(), ["spawned from a job"]);
    }
}
