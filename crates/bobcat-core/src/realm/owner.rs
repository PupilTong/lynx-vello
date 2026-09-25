//! Driving a realm once it is open: the one driver a view's page on
//! `bobcat-main` and a worker on `bobcat-workers` both run.
//!
//! Each of the two owns one realm and drives it the same way. Its tasks are
//! spawned under a guard that ends it if one of them unwinds; each task
//! reaches the realm through one job at a time, which runs one synchronous
//! operation and then the epilogue; one latch ends it, and one report says
//! why; and the owner's own task waits for that end, reclaims the tasks and
//! releases the realm. [`RealmOwner`] is what each of the two supplies —
//! where its runtime, its realm and its [`Lifetime`] are, where what it
//! reports goes, and what its role adds to an entry, to the end and to the
//! release — and the functions here are the rest, written once.
//!
//! # Entries
//!
//! [`enter`] is the one way into an owner's realm: it queues one job and
//! answers with what the operation returned. [`enter_now`] is that job's body.
//! The job that opens a realm cannot go through [`enter`], because the realm
//! does not exist until that job stores it, and runs [`enter_now`] inline once
//! it has, for the realm's first epilogue. [`after_end`] is the job for what
//! runs against a realm after the end, past the latch and the epilogue.
//!
//! # The end
//!
//! [`end`] sets the latch, once, and runs what the owner's role owes an end.
//! [`terminal`] reports why an owner is over and ends it: a view's
//! `StartupFailed`, a worker's `Failed`, or the `Closed` of a worker that
//! closed itself. [`trapped`] reports a panic and ends the owner. Each report
//! goes through a latch of its own on the lifetime, so one end is one report
//! and a panic after it is still reported.
//!
//! A panic that unwinds an owner's own task — `serve_view` or `serve_worker` —
//! or a whole engine thread is not reported here: the `Rc` of the owner that
//! task held is dropped with it, so the report is the thread's, read from the
//! thread's own table of who to tell.

use std::any::Any;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use crate::lifetime::{EndOnUnwind, Lifetime, Settles, run_job};
use crate::main::quickjs::{ScriptRuntime, SharedRuntime};

/// What a realm-owning object supplies to the driver: where its runtime, its
/// realm and its lifetime are, where what it reports goes, and what its role
/// adds to an entry, an end and a release.
///
/// Implemented by the view's `Page` and the worker's `Worker`, which is every
/// realm-owning object there is.
pub(crate) trait RealmOwner: Sized + 'static {
    /// The realm this owner drives: a view's `MainThreadRuntime`, or a
    /// worker's `WorkerRealm`.
    type Realm: 'static;
    /// What this owner reports: an `EngineEvent` to the view's host, or a
    /// `WorkerPayload` to the realm that created the worker.
    type Event;

    /// Every task of this owner, the token that ends them, and the latches
    /// its end and its reports go through.
    fn lifetime(&self) -> &Lifetime;

    /// The script runtime every realm on this owner's thread shares, or why
    /// it could not be built.
    fn runtime(&self) -> &SharedRuntime;

    /// This owner's realm: `None` before the job that opens it has run, and
    /// again once it could not be opened or the owner has released it.
    ///
    /// Read and written only inside a job, which holds this borrow for its
    /// whole length, its own synchronous wait included.
    fn realm(&self) -> &RefCell<Option<Box<Self::Realm>>>;

    /// Sends one report to where this owner's reports go.
    fn send(&self, event: Self::Event);

    /// Everything one entry into the realm leaves owing, run under the
    /// entry's own borrows once its operation has returned.
    fn epilogue(owner: &Rc<Self>, realm: &mut Self::Realm, js: &mut ScriptRuntime);

    /// What a panic of this owner's is reported as.
    fn panic_event(payload: &(dyn Any + Send)) -> Self::Event;

    /// What this owner's role owes the end beyond the lifetime's own, run
    /// once, by the call to [`end`] that ended it. It touches no realm borrow,
    /// because a task may end the owner at any time.
    fn on_end(&self) {}

    /// What this owner's role does once the end has been reached and every
    /// task reaped, before the realm is released: jobs through [`after_end`],
    /// each awaited.
    fn before_release(_owner: &Rc<Self>) -> impl Future<Output = ()> {
        std::future::ready(())
    }
}

/// Starts one more task of `owner`'s.
///
/// A panic anywhere in the task ends the owner during the unwind: the guard's
/// `Drop` runs before tokio's task harness catches the panic, so a sibling
/// task polled before the owner's own task already finds the latch set, and
/// what [`end`] owes has already happened. The report is the owner task's,
/// out of the payload the lifetime hands it in [`run_owner`].
pub(crate) fn spawn<O: RealmOwner>(owner: &Rc<O>, future: impl Future<Output = ()> + 'static) {
    let guard = EndOnUnwind::new(owner);
    owner.lifetime().spawn(async move {
        let _guard = guard;
        future.await;
    });
}

/// Ends `owner`, once, and runs [`RealmOwner::on_end`] for the call that did.
///
/// Synchronous, because the latch is what makes an end immediate: [`enter`]
/// and every timer wake return at once when it is set, and the owner task's
/// abort only reclaims the tasks. The deadline the realm had armed is
/// withdrawn by the lifetime's own end.
///
/// The owner task calls it after its wait too, which is how a cancellation
/// that arrived from another thread reaches the latch: nothing but this thread
/// writes it.
pub(crate) fn end<O: RealmOwner>(owner: &Rc<O>) {
    if owner.lifetime().end() {
        owner.on_end();
    }
}

/// Reports why `owner` is over and ends it, in that order and once.
///
/// `event` is always an end: a view's `StartupFailed`, a worker's `Failed`,
/// or a worker's `Closed`. The first of them is the one reported, through the
/// lifetime's [`report_terminal`](Lifetime::report_terminal) latch. A panic is
/// [`trapped`]'s, through a latch of its own.
///
/// It touches no realm borrow, so an operation may call it from inside
/// [`enter`]: the epilogue that follows sees the end and does nothing, which
/// is what keeps one failure one report.
pub(crate) fn terminal<O: RealmOwner>(owner: &Rc<O>, event: O::Event) {
    if owner.lifetime().report_terminal() {
        owner.send(event);
    }
    end(owner);
}

/// Reports a panic as `owner`'s, and ends it.
///
/// One report per owner, whichever path saw the panic first — the job that
/// caught one inside [`enter`], or the owner task reaping a task that
/// trapped — and it is not gated by [`terminal`]'s latch: an owner that
/// already reported why it ended and then traps still says so.
pub(crate) fn trapped<O: RealmOwner>(owner: &Rc<O>, payload: &(dyn Any + Send)) {
    if owner.lifetime().report_panic() {
        owner.send(O::panic_event(payload));
    }
    end(owner);
}

/// Queues one synchronous operation against `owner`'s live realm, which then
/// settles what it owes, and answers with what the operation returned.
///
/// This is the one way into JavaScript. `None` is a realm that is not live,
/// an owner that has ended, an operation that trapped, or a thread that is
/// over; either way nothing of the operation is observable here.
///
/// The operation *and* the whole epilogue run under one `catch_unwind` inside
/// [`run_job`], because a panic on an engine thread is the owner's failure
/// rather than the thread's, and a job runs in the thread's top loop rather
/// than inside a task that could catch it. A host function that panics is one
/// of these panics too: the script is shown an exception, and the realm's
/// checkpoint resumes the panic when the operation that called it ends.
pub(crate) fn enter<O, T, F>(
    owner: &Rc<O>,
    operation: F,
) -> impl Future<Output = Option<T>> + use<O, T, F>
where
    O: RealmOwner,
    T: 'static,
    F: FnOnce(&mut O::Realm, &mut ScriptRuntime) -> T + 'static,
{
    run_job(owner, move |owner| enter_now(owner, operation))
}

/// The body of one entry, as the job runs it.
///
/// Both borrows are held for the whole entry, a synchronous wait inside the
/// operation included. Nothing else can want them: only a job takes either,
/// and no other job runs until this one returns.
pub(crate) fn enter_now<O: RealmOwner, T>(
    owner: &Rc<O>,
    operation: impl FnOnce(&mut O::Realm, &mut ScriptRuntime) -> T,
) -> Option<T> {
    if owner.lifetime().ended() {
        return None;
    }
    let mut js = owner.runtime().borrow_mut();
    let Ok(js) = js.as_mut() else {
        return None;
    };
    let mut realm = owner.realm().borrow_mut();
    let realm = realm.as_deref_mut()?;
    let value = operation(realm, js);
    O::epilogue(owner, realm, js);
    Some(value)
}

/// One job against `owner`'s realm after the owner has ended: no latch, and
/// no epilogue.
///
/// The owner is over by the time any of these runs — there is nothing left to
/// commit, report or re-arm — so it is [`RealmOwner::before_release`] and the
/// release in [`run_owner`] that use it, and nothing else. The realm is still
/// in [`RealmOwner::realm`] throughout, which is what puts the release behind
/// whatever job is holding it. On a runtime that was never built, which opened
/// no realm, the operation does not run and the answer is `None`, as for a job
/// that never ran.
pub(crate) fn after_end<O, T, F>(
    owner: &Rc<O>,
    operation: F,
) -> impl Future<Output = Option<T>> + use<O, T, F>
where
    O: RealmOwner,
    T: 'static,
    F: FnOnce(&mut Option<Box<O::Realm>>, &mut ScriptRuntime) -> T + 'static,
{
    run_job(owner, move |owner| {
        let mut js = owner.runtime().borrow_mut();
        let Ok(js) = js.as_mut() else {
            return None;
        };
        Some(operation(&mut owner.realm().borrow_mut(), js))
    })
}

/// `owner`'s whole tail: wait, end, reclaim, what its role does before the
/// release, and the release of its realm.
///
/// The wait is the lifetime's — the end, or the next task of the owner to
/// finish. [`end`] after it is what mirrors a cancellation that came from
/// another thread onto this thread's latch, and so what runs
/// [`RealmOwner::on_end`] for an end nothing on this thread had reached yet. A
/// task that panicked hands its payload to [`trapped`]. The reap is what makes
/// this the last holder of the owner: a task holds an `Rc` of it until its
/// future is dropped.
///
/// What follows is jobs, every one of them awaited. That is what keeps the
/// release from landing on a realm that is under a live JavaScript stack: a
/// job of this owner's that is parked on a synchronous wait is ahead of the
/// release in the one FIFO, so the release cannot begin until it has returned.
pub(crate) async fn run_owner<O: RealmOwner>(owner: &Rc<O>) {
    owner
        .lifetime()
        .serve(&mut |payload| trapped(owner, payload.as_ref()))
        .await;
    end(owner);
    owner
        .lifetime()
        .reap(&mut |payload| trapped(owner, payload.as_ref()))
        .await;
    O::before_release(owner).await;
    after_end(owner, |realm, _| *realm = None).await;
}

/// What an owner's clock task, its unwind guard and its jobs reach it
/// through, for every owner there is.
impl<O: RealmOwner> Settles for O {
    fn lifetime(&self) -> &Lifetime {
        RealmOwner::lifetime(self)
    }

    /// The epilogue alone, for a wake that carries no operation of its own —
    /// a timer deadline, or a sibling's checkpoint.
    fn settle(owner: &Rc<Self>) -> impl Future<Output = Option<()>> {
        enter(owner, |_, _| ())
    }

    fn end(owner: &Rc<Self>) {
        end(owner);
    }

    fn trapped(owner: &Rc<Self>, payload: &(dyn Any + Send)) {
        trapped(owner, payload);
    }
}
