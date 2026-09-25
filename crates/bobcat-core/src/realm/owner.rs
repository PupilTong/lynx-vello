//! Driving a realm once it is open: the one driver a view's page on
//! `bobcat-main` and a worker on `bobcat-workers` both run.
//!
//! Each of the two owns one realm and drives it the same way. Its tasks are
//! spawned under a guard that ends it if one of them unwinds; each task
//! reaches the realm through one job at a time, which runs one synchronous
//! operation and then the epilogue; one latch ends it, and one report says
//! why; and the owner's own task waits for that end, reclaims the tasks and
//! releases the realm. [`RealmOwner`] is what each of the two supplies —
//! where its runtime, its realm, its host and its [`Lifetime`] are, where
//! what it reports goes, which of the tables in [`policy`] its failures are
//! read from, and what its role adds to an entry, to the epilogue, to the end
//! and to the release — and the functions here are the rest, written once.
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
//! # The epilogue
//!
//! Every entry ends in [`epilogue`], one function for both kinds of owner,
//! which settles what the entry left owing. The steps both kinds have are
//! written there; what only one role has is a hook of [`RealmOwner`]'s, run at
//! a fixed place among them, and a hook a role does not need is empty. The
//! order of the steps is the contract, and [`epilogue`] lists it.
//!
//! The imports an entry left waiting and the futures it asked to settle are
//! each a task the epilogue spawns — [`load_module`] and [`settle_future`] —
//! which waits for its answer outside any job and then enters the realm to
//! hand it over. [`module_answer`] is the one reading of a module request's
//! answer, for those loads and for the entry a role completes itself.
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
//! thread's own table of who to tell. The event it sends is built by the
//! same Panic row [`trapped`] reports through.
//!
//! Every failure the driver itself meets — a timer callback, a module load, a
//! future's settle, and the root module's rejection where the owner's
//! [`RealmOwner::BOOT_REJECTION`] names a scene — is reported through
//! [`policy::report`], under the [`Scene`] it happened in.

use std::any::Any;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use tokio::sync::oneshot::error::RecvError;

use super::RealmCore;
use super::policy::{self, Row, Scene};
use crate::future::HostFuture;
use crate::lifetime::{EndOnUnwind, Lifetime, Settles, run_job};
use crate::link::{HostOutbox, SourceAnswer};
use crate::main::quickjs::{ScriptRuntime, SharedRuntime};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::script::ScriptError;
use crate::view::LynxViewError;

/// What a realm-owning object supplies to the driver: where its runtime, its
/// realm, its host and its lifetime are, where what it reports goes and which
/// table decides it, and what its role adds to an entry, the epilogue, an end
/// and a release.
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

    /// The core every realm has, out of this owner's realm: the engine and
    /// the timer and future tables its core members write to.
    fn core(realm: &mut Self::Realm) -> &mut RealmCore;

    /// What this owner's realm asks the view's host through, carrying the
    /// token its requests are cancelled with: the view's for a page, the
    /// worker's own for a worker.
    fn host(&self) -> &HostOutbox;

    /// Sends one report to where this owner's reports go.
    fn send(&self, event: Self::Event);

    /// This owner's realm kind's row for `scene`: what a failure there is
    /// reported as, whether it ends the owner, and what it is prefixed with.
    /// One of the tables in [`policy`], which [`policy::report`] reads.
    fn row(scene: Scene) -> Row<Self::Event>;

    /// This owner's realm kind's Panic row, which [`trapped`] reports
    /// through.
    fn panic_row() -> Row<Self::Event>;

    /// Runs every timer of `realm`'s that has come due, and answers with what
    /// their callbacks threw. The first thing the [`epilogue`] runs.
    fn run_due_timers(realm: &mut Self::Realm, js: &mut ScriptRuntime) -> Vec<ScriptError>;

    /// What this owner's role settles right after the due timers, before the
    /// boot report, for an owner those timers' callbacks did not end. It may
    /// end the owner, and the [`epilogue`] stops there when it does.
    fn after_timers(_owner: &Rc<Self>, _realm: &mut Self::Realm) {}

    /// Whether the realm's root module has already been seen to finish.
    fn booted(&self) -> bool;

    /// Records that the realm's root module has finished, or rejected.
    fn mark_booted(&self);

    /// What this owner's role reports once its root module has finished,
    /// which is once.
    fn on_booted(&self) {}

    /// The scene a rejection of the realm's root module is reported under,
    /// and the context the [`epilogue`] names it by first, for a row that
    /// adds none of its own. `None` for a realm kind whose epilogue reads the
    /// root module only to learn that it has settled, because the entry the
    /// rejection happened in has already reported it.
    const BOOT_REJECTION: Option<(Scene, Option<&'static str>)>;

    /// What this owner's role settles once the boot report is behind it.
    fn after_boot(&self) {}

    /// The name of the one module request of this realm's that the role
    /// answers itself, from an answer it already holds. The [`epilogue`]
    /// never asks the host for it.
    fn entry_name(&self, _realm: &Self::Realm) -> Option<String> {
        None
    }

    /// What this owner's role settles once the module loads and the future
    /// settles are spawned, before the next deadline is armed.
    fn after_settles(_owner: &Rc<Self>, _realm: &mut Self::Realm) {}

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
        owner.send(O::panic_row().event_for_panic(payload));
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
    epilogue(owner, realm, js);
    Some(value)
}

/// Everything one entry into `owner`'s realm leaves owing, run under the
/// entry's own borrows once its operation has returned.
///
/// The order is the contract:
///
/// 1. **Nothing, for an owner that has ended** — whether the operation ended it or a task did
///    during a synchronous wait inside it. The end has already done what an end owes.
/// 2. **The count**, in tests.
/// 3. **Due timers.** A timer that has come due runs before the role's next step, so on a page its
///    mutation rides the same commit as whatever else this entry changed. Each callback that threw
///    is reported under [`Scene::Timer`].
/// 4. **Nothing more, for an owner that ended while those callbacks ran**: a callback parked on a
///    synchronous wait lets the thread's tasks run, and one of them may end the owner — a
///    `Terminate` its consumer read, a release, a panic in another of its tasks. The callbacks of
///    the batch after that one still run, and what they threw is still reported.
/// 5. **[`RealmOwner::after_timers`]**: a page's commit and the deliveries it posts; a worker's
///    `close()`, which ends it.
/// 6. **Nothing more, for an owner that step ended.**
/// 7. **The boot report**, until the realm's root module has settled: a finished module is marked
///    and reported through [`RealmOwner::on_booted`], after the commit, so the frame exists before
///    the event that implies it; a rejected one is marked and, where [`RealmOwner::BOOT_REJECTION`]
///    names a scene, reported under it.
/// 8. **Nothing more, for an owner that report ended.**
/// 9. **[`RealmOwner::after_boot`]**: a page's `BeginFrame` acknowledgement, after both the commit
///    and the boot report, because a host blocked on that sequence number is blocked on the frame.
/// 10. **The module requests** this entry produced, each asked of the host and spawned as a
///     [`load_module`] of its own — except the one [`RealmOwner::entry_name`] names, which the role
///     answers itself.
/// 11. **The futures** a `.then` asked this realm to settle, each spawned as a [`settle_future`] of
///     its own.
/// 12. **[`RealmOwner::after_settles`]**: a page's `@font-face` loads.
/// 13. **The next timer deadline**, republished only when it moved.
/// 14. **The checkpoint generation**, last, so it names the generation this entry ran the shared
///     job queue up to: the clock task compares a bump against it to tell this owner's own entries
///     from a sibling's.
///
/// Every step after an end is skipped, not only the reports: an owner that has
/// ended asks the host for nothing, spawns nothing and re-arms no deadline its
/// end withdrew.
fn epilogue<O: RealmOwner>(owner: &Rc<O>, realm: &mut O::Realm, js: &mut ScriptRuntime) {
    let lifetime = owner.lifetime();
    if lifetime.ended() {
        return;
    }
    #[cfg(test)]
    lifetime.count_epilogue();
    for error in O::run_due_timers(realm, js) {
        policy::report(owner, Scene::Timer, error);
    }
    if lifetime.ended() {
        return;
    }
    O::after_timers(owner, realm);
    if lifetime.ended() {
        return;
    }
    if !owner.booted() {
        match O::core(realm).engine.module_finished() {
            Ok(false) => {}
            Ok(true) => {
                owner.mark_booted();
                owner.on_booted();
            }
            Err(error) => {
                owner.mark_booted();
                if let Some((scene, context)) = O::BOOT_REJECTION {
                    let error = match context {
                        Some(context) => policy::context_of(context, error),
                        None => error,
                    };
                    policy::report(owner, scene, error);
                }
            }
        }
    }
    if lifetime.ended() {
        return;
    }
    owner.after_boot();
    while let Some(url) = O::core(realm).engine.take_module_request() {
        // The entry's own request was made before the realm asked for it, and
        // the role completes it from that answer: completing it is what
        // resumes the load this request stands for, and it must never reach
        // the fetcher a second time.
        if owner.entry_name(realm).is_some_and(|entry| entry == url) {
            continue;
        }
        let answer = owner.host().request(SourceRequest::Module(url.clone()));
        spawn(owner, load_module(Rc::clone(owner), url, answer));
    }
    for (id, future) in O::core(realm).futures.take_settle_requests() {
        spawn(owner, settle_future(Rc::clone(owner), id, future));
    }
    O::after_settles(owner, realm);
    lifetime.arm_deadline(O::core(realm).timers.next_deadline());
    lifetime.record_checkpoint(js.checkpoint_generation());
}

/// One resource load an import of `owner`'s realm produced.
///
/// The load's outcome is the module's: a load that failed, or an answer that
/// is not a script, completes the module with an error naming it, which
/// rejects the import in the realm, where the code that made it can catch it.
/// What the completion itself returns — the realm refusing it, or a rejection
/// the code it resumed left unhandled — is reported under [`Scene::Module`],
/// whether the realm has booted or not: the import is the app's, and so is
/// whatever awaited it.
///
/// A successful answer completes the module under `url`, the name the import
/// asked for; the URL the fetcher answered from is the module's own URL — its
/// `import.meta.url`, and the base its own imports resolve against.
async fn load_module<O: RealmOwner>(owner: Rc<O>, url: String, answer: SourceAnswer) {
    let loaded = await_source(answer)
        .await
        .map_err(|error| error.to_string())
        .and_then(|answer| module_answer(&url, answer))
        .map_err(|reason| format!("module '{url}': {reason}"));
    let completing = Rc::clone(&owner);
    enter(&owner, move |realm, js| {
        let loaded = loaded
            .as_ref()
            .map(|(response, source)| (response.as_str(), source.as_str()))
            .map_err(String::as_str);
        if let Err(error) = O::core(realm).engine.complete_module(js, &url, loaded) {
            policy::report(&completing, Scene::Module, error);
        }
    })
    .await;
}

/// One future a `.then` asked `owner`'s realm to settle.
///
/// Its shape is [`load_module`]'s, and so is what a failure costs: the realm
/// refusing the settle, or the code it resumed failing, is reported under
/// [`Scene::Future`], and the realm goes on.
async fn settle_future<O: RealmOwner>(owner: Rc<O>, id: u32, future: HostFuture) {
    let outcome = future.await;
    let settling = Rc::clone(&owner);
    enter(&owner, move |realm, js| {
        let delivered = crate::future::deliver(&mut O::core(realm).engine, js, id, outcome);
        if let Err(error) = delivered {
            policy::report(&settling, Scene::Future, error);
        }
    })
    .await;
}

/// Waits for one source the host was already asked for. A completion the
/// fetcher dropped without answering is a failed load, not a wait forever.
///
/// It takes the answer's receiving end or a borrow of it, so a caller that
/// waits for the answer inside a `select!` keeps it across the turns another
/// arm wins.
pub(crate) async fn await_source(
    answer: impl Future<Output = Result<Result<LoadedSource, LynxViewError>, RecvError>>,
) -> Result<LoadedSource, LynxViewError> {
    answer
        .await
        .unwrap_or_else(|_| Err(unanswered_source().into()))
}

/// The script an answer to the module request `requested` carries — the URL
/// the fetcher answered from, and the source — or, for an answer of another
/// kind, the text `the fetcher returned a <kind> for <requested>`.
///
/// The one reading of such an answer, for every module a realm imports and
/// for the entry a role completes itself. A load that failed is not read
/// here: an import and a plain `Worker`'s script pass the fetcher's own error
/// on as its text, and the MTS entry passes it on as the `LynxViewError` it
/// is and makes this text a `Script` error.
pub(crate) fn module_answer(
    requested: &str,
    answer: LoadedSource,
) -> Result<(String, String), String> {
    let kind = match answer {
        LoadedSource::Module { source, url } => return Ok((url, source)),
        LoadedSource::StyleSheet(_) => "stylesheet",
        LoadedSource::Font(_) => "font",
        LoadedSource::Fetched => "plain fetch",
    };
    Err(format!("the fetcher returned a {kind} for {requested}"))
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

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;
    use crate::resource::{ResourceErrorKind, StyleSheetSource};
    use crate::test_support::block_on;

    const REQUESTED: &str = "app:///dep.js";

    #[test]
    fn a_script_answer_is_its_response_url_and_its_source() {
        let answered = module_answer(
            REQUESTED,
            LoadedSource::Module {
                source: "export {};".to_owned(),
                url: "app:///redirected.js".to_owned(),
            },
        );
        let Ok((url, source)) = answered else {
            panic!("a script answer is read as one: {answered:?}");
        };
        assert_eq!(url, "app:///redirected.js");
        assert_eq!(source, "export {};");
    }

    /// Every answer that is not a script is refused the same way, naming the
    /// request and the kind it was answered with, and nothing else: the text
    /// is what an import is rejected with and what a worker's `Failed` says.
    #[test]
    fn an_answer_of_another_kind_is_refused_naming_the_request_and_the_kind() {
        for (answer, kind) in [
            (
                LoadedSource::StyleSheet(StyleSheetSource::Text(String::new())),
                "stylesheet",
            ),
            (
                LoadedSource::Font(dom::FontBlob::from_static(b"not a font")),
                "font",
            ),
            (LoadedSource::Fetched, "plain fetch"),
        ] {
            let answered = module_answer(REQUESTED, answer);
            assert_eq!(
                answered,
                Err(format!("the fetcher returned a {kind} for {REQUESTED}"))
            );
        }
    }

    /// A completion dropped without ever sending, which a cancelled one is,
    /// is the same failure a completion that dropped unanswered sends.
    #[test]
    fn an_answer_that_never_comes_is_an_unanswered_source() {
        let (completion, answer) = oneshot::channel();
        drop(completion);
        let answered = block_on(await_source(answer));
        let Err(LynxViewError::Resource(error)) = answered else {
            panic!("a dropped completion is a failed load: {answered:?}");
        };
        assert!(matches!(error.kind, ResourceErrorKind::Unavailable));
        assert_eq!(error.message, unanswered_source().message);
    }
}
