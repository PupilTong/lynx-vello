//! One view on `bobcat-main`: the page every task of that view acts on, and
//! the one boundary each of them enters JavaScript through.
//!
//! # Tasks wait; jobs run JavaScript
//!
//! `bobcat-main` is one engine thread of [`crate::jobs`], whose module doc is
//! the model: nothing here runs JavaScript, touches this view's document or
//! borrows the group's `ScriptRuntime` from inside a task.
//!
//! # One task per wait
//!
//! A view is not one task with a wait for everything. It is a set of tasks on
//! the thread's `LocalSet`, one per thing that can be waited for, and tokio is
//! what polls, parks and wakes them:
//!
//! - the owner, [`serve_view`], which builds the page, spawns the rest, and then has exactly one
//!   wait of its own — the view's [`Lifetime`], which is the end or the next task of the view to
//!   finish;
//! - [`consume_commands`], the one ordered consumer of the command stream;
//! - [`boot_page`], the page's boot future: the sheets in cascade order, the entry, and then the
//!   realm;
//! - one [`load_module`] future per resource load an import produced;
//! - one [`settle_future`] per host-backed `Future` a `.then` asked this realm to settle;
//! - [`consume_worker_events`], the one ordered consumer of this view's workers;
//! - [`serve_clock`], which owns this realm's one pinned sleep and watches the runtime-wide
//!   checkpoint generation for a sibling's entry into JavaScript.
//!
//! Nothing is spawned per input. An ordered stream stays serial because one
//! task consumes it with `while let Some(x) = rx.recv().await` — a consumer,
//! not a scheduler.
//!
//! # One boundary into JavaScript
//!
//! Every one of those tasks reaches the realm through [`Page::enter`], which
//! queues one job and answers with what it returned. The job runs one
//! synchronous operation and then settles what that operation left owing: due
//! timers, the commit, the boot report, the `BeginFrame` acknowledgement, the
//! module requests the entry produced, the next timer deadline.
//! [`Settles::settle`] is the epilogue alone, for a wake that carries no
//! operation of its own. [`Page::open_realm`] is a job too, and the only one
//! that does not go through `enter`, because the realm it would enter does not
//! exist until it returns; the disposal exchange in [`Page::run_owner`] is the
//! other exception, running after the view has ended and so past the latch and
//! the epilogue.
//!
//! What is *not* a job is what a page that is still loading does with a burst:
//! its ingredients are a field of their own, never borrowed across a wait, so
//! [`Page::stage`] and [`Page::stage_sheet`] answer on the task. That is what
//! keeps a loading page's `BeginFrame` acknowledgement from queueing behind a
//! sibling view's synchronous load.
//!
//! # Ordering
//!
//! Jobs are one FIFO for the whole thread and each of them is synchronous, so
//! entries never interleave: each stream is consumed in order by its one
//! consumer, and a burst of commands is one entry, one commit and one
//! acknowledgement. Module completions, timer wakes and a sibling's checkpoint
//! are independent tasks and may queue an entry between any two bursts.
//!
//! # The end
//!
//! One [`Lifetime`] per view carries the tasks, the token that ends them and
//! the latch this thread reads. The token is the embedder's: `LynxView::drop`
//! and a fatal `pump` event cancel it, [`serve_view`]'s drop guard cancels it
//! on every exit. Worker tokens are independent: after ordinary view tasks
//! stop, MTS completes JS disposal over the same Worker inbox before releasing
//! its realm. What ended a view is not recorded anywhere, because nothing reads it:
//! what the embedder was told is whatever was reported before the end, and a
//! release is the token having been cancelled from outside.
//!
//! Between an embedder-side cancel and the owner's turn a task may still queue
//! one entry, because only this thread writes the latch. A command queued
//! behind a release lands in exactly that window — its wake is served before
//! the owner's — so the discard cannot rest on the owner running first:
//! [`consume_commands`] reads the token at the wake boundary, and the burst's
//! own job reads it again as it starts, because the top loop runs queued jobs
//! back to back and the owner may not have mirrored the cancel yet. A burst
//! already inside its entry when the cancel lands finishes, the way
//! synchronous JavaScript already executing does.
//!
//! # Borrows
//!
//! No `RefCell` borrow and no borrow of the shared script runtime is ever held
//! across an `.await`, and none is ever taken by a task. A job holds the
//! shared runtime and this page's realm for its whole length, its own
//! synchronous wait included; [`crate::jobs`] is what makes that safe. The
//! page's ingredients are the exception in the other direction: they belong to
//! the loading tasks, so no job holds them across a wait.
//!
//! # Waits
//!
//! After this module every `select!` in this crate is one of five kinds, and
//! each is a wait rather than a dispatcher:
//!
//! - **the top loop's turn** — one per engine thread, inside
//!   [`JsThread::run`](crate::jobs::JsThread), waiting on its `main` task finishing versus a job
//!   having been pushed;
//! - **thread lifetime** — `group_task` and `serve_workers`, each waiting on attach versus join;
//! - **an object's lifetime** — one [`Lifetime::serve`] per view and per worker, waiting on the end
//!   versus the next task of that object to finish;
//! - **a realm's clock** — one [`serve_clock`] per live realm, a view's and a worker's alike,
//!   waiting on its deadline, the re-arm that moves it, and a sibling's checkpoint;
//! - **the worker's pre-boot wait** — its script versus termination, channel closure, or its own
//!   cancellation.
//!
//! How many there are is the group's shape rather than a constant: one of the
//! first two kinds per engine thread, one of the third per live view and per
//! live worker, one of the fourth per live realm, one of the fifth per worker
//! that has not booted yet. `link.rs`'s `block_on_deadline` is a hand-rolled poll
//! loop rather than a select, and the only one left. The two synchronous
//! host members are a fifth wait of their own shape — this view's token
//! against the answer — parked on inside a job through
//! [`JsThread::wait`](crate::jobs::JsThread): stylesheet adoption, and
//! [`crate::future`]'s `waitFuture`, which adds an optional deadline behind
//! the token.

use std::cell::{Cell, RefCell};
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::quickjs::ScriptRuntime;
use super::runtime::{DocumentIngredients, MainThreadRuntime, RealmStartup};
use super::{AttachedView, GroupContext};
use crate::background::WorkerEvent;
#[cfg(test)]
use crate::clock::ClockInstant;
use crate::lifetime::{EndOnUnwind, Lifetime, Settles, run_job, serve_clock};
use crate::link::{SourceAnswer, ToMain, ViewOutbox};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::threads::panicked;
use crate::view::{EngineEvent, LynxViewError, ViewSources, Viewport};

/// One view's page: what every task of that view acts on.
///
/// Shared as an `Rc` rather than owned by one task, because a view has as
/// many tasks as it has things to wait for and each of them acts on this.
/// `!Send` and `!Sync` by construction — it holds the group's `Rc`s and,
/// through the realm, a document whose arena backpointer is a raw pointer.
pub(super) struct Page {
    /// What every view on this thread shares: the script runtime, the style
    /// pool, the wakeup, and the factory that names workers.
    context: Rc<GroupContext>,
    outbox: ViewOutbox,
    /// This view's realm, once the boot module has opened one: `None` before
    /// that, and again once the realm could not be opened or the owner has
    /// released it. It is the only thing that decides what a command can do to
    /// this page.
    ///
    /// Read and written only inside a job, which holds this borrow for its
    /// whole length, its own synchronous wait included. What a *task* asks
    /// instead is [`Page::is_loading`], over the ingredients.
    ///
    /// Boxed because this is one field of a page every task of the view holds,
    /// and a page is live for exactly one phase of its life.
    realm: RefCell<Option<Box<MainThreadRuntime>>>,
    /// What the realm's document will be built from, until [`Page::open_realm`]
    /// spends it.
    ///
    /// A field of its own rather than a part of [`Self::realm`], because this
    /// is the one thing a *task* of a loading view writes: the sheets boot
    /// staged, a resize, the image reports that arrived first. No job holds
    /// this borrow across a wait, so a burst for a page that is still loading
    /// — and the `BeginFrame` acknowledgement an offscreen host is blocked on
    /// — is served at once, whatever a sibling view's job is parked on.
    ingredients: RefCell<Option<Box<DocumentIngredients>>>,
    /// The same inbox serves ordinary events and the final JS disposal RPC.
    /// The owner takes it only after the ordinary consumer has been reaped.
    worker_events: RefCell<Option<mpsc::UnboundedReceiver<WorkerEvent>>>,
    /// The newest `BeginFrame` sequence applied and not yet acknowledged.
    ///
    /// Acknowledged in the epilogue rather than where it is applied: a host
    /// blocked on this number is waiting for the frame it implies, which the
    /// epilogue's commit is what publishes.
    pending_begin_frame: Cell<Option<u64>>,
    boot_reported: Cell<bool>,
    /// Whether an entry that will deliver `dom`'s queued
    /// `contentvisibilityautostatechange` changes is already queued and has
    /// not run yet.
    ///
    /// The queue outlives the epilogue that noticed it — the drain is the
    /// delivery entry's, because the point of the entry is that the walk does
    /// not run inside the commit — so without this latch every entry between
    /// the two would post one of its own and each would find the batch
    /// already gone. One post per batch.
    content_visibility_posted: Cell<bool>,
    /// Whether an entry that will deliver the `<image>` `load`s and `error`s
    /// the runtime has queued is already queued and has not run yet.
    ///
    /// The same latch as [`Self::content_visibility_posted`], for the same
    /// reason: the drain belongs to the delivery entry, so every entry that
    /// runs between the post and it would otherwise post one more.
    image_outcomes_posted: Cell<bool>,
    /// Every task of this view, the token that ends them, the latch this thread
    /// reads, and the two numbers this realm's clock task waits on — the
    /// deadline it armed and the generation its own last entry recorded.
    lifetime: Lifetime,
    /// Whether this view has already been told why it failed. The first report
    /// wins, so one failure is one `StartupFailed` or one `ScriptRunError`. A
    /// panic is not gated here — it has a latch of its own on the lifetime,
    /// because a task that traps after a startup failure is still a fact the
    /// embedder is owed.
    reported: Cell<bool>,
    /// How many times the epilogue has run, for the tests that count the
    /// wakes a page answers.
    #[cfg(test)]
    epilogues: Cell<u64>,
}

/// Everything boot still needs once the fonts have been validated and the
/// document's ingredients staged: the pre-fetch form, mirroring
/// [`ViewSources`], of what becomes one
/// [`RealmStartup`](super::runtime::RealmStartup) as soon as the entry has
/// arrived — the sheets and the entry *specifier* here, the entry's own text
/// and resolved URL there.
struct BootSources {
    style_sheets: Vec<String>,
    entry: String,
    background_entry: Option<String>,
    /// The host's page data, as JSON text only the realm reads.
    init_data: Option<String>,
    initial_processor: String,
    global_props: Option<String>,
    /// The embedder's native modules, already encoded as the record the MTS
    /// realm reads their names and methods out of.
    native_modules: String,
}

impl Page {
    /// A loading page, over the token that ends it.
    fn new(
        context: Rc<GroupContext>,
        outbox: ViewOutbox,
        ingredients: DocumentIngredients,
        token: CancellationToken,
    ) -> Rc<Self> {
        let lifetime = Lifetime::new(token, context.thread.clone());
        Rc::new(Self {
            context,
            outbox,
            realm: RefCell::new(None),
            ingredients: RefCell::new(Some(Box::new(ingredients))),
            worker_events: RefCell::new(None),
            pending_begin_frame: Cell::new(None),
            boot_reported: Cell::new(false),
            content_visibility_posted: Cell::new(false),
            image_outcomes_posted: Cell::new(false),
            lifetime,
            reported: Cell::new(false),
            #[cfg(test)]
            epilogues: Cell::new(0),
        })
    }

    /// Starts one more task of this view.
    ///
    /// A panic anywhere in the task ends the view: the guard's `Drop` runs
    /// during the unwind, before tokio's task harness catches it, so a sibling
    /// task polled before the owner already finds [`Self::ended`] true — and
    /// the acknowledgement and the withdrawn deadline that [`Self::end`] owes
    /// have already happened. The report is the owner's, out of the payload
    /// the lifetime hands it.
    fn spawn(self: &Rc<Self>, future: impl Future<Output = ()> + 'static) {
        let guard = EndOnUnwind::new(self);
        self.lifetime.spawn(async move {
            let _guard = guard;
            future.await;
        });
    }

    /// Ends this view, once. `true` for the call that did it.
    ///
    /// Synchronous, because the latch is what makes an end immediate:
    /// [`Self::enter`], [`Self::apply`] and every timer wake return at once
    /// when it is set, and the owner's abort only reclaims the tasks.
    ///
    /// The owner calls it after its wait too, which is how a cancellation that
    /// arrived from another thread reaches the latch: nothing but this thread
    /// writes it.
    fn end(&self) -> bool {
        if !self.lifetime.end() {
            return false;
        }
        // A painter blocked in `wait_begin_frame` is released rather than
        // timed out: the frame it was waiting for will never come. The deadline
        // the realm had armed is withdrawn by the lifetime's own end above.
        if let Some(seq) = self.pending_begin_frame.take() {
            self.outbox.begin_frame_serviced(seq);
        }
        true
    }

    fn ended(&self) -> bool {
        self.lifetime.ended()
    }

    /// Reports one fatal failure and ends the view, in that order and once.
    ///
    /// It touches no realm borrow, so an operation may call it from inside
    /// [`Self::enter`]: the epilogue that follows sees the end and does
    /// nothing, which is what keeps one failure one report.
    fn fail(&self, event: EngineEvent) {
        if !self.reported.replace(true) {
            self.outbox.engine_event(event);
        }
        self.end();
    }

    /// Whether the realm's ingredients are still staged, which is the one
    /// question about a page's phase a *task* may ask: the answer is a borrow
    /// no job holds across a wait.
    fn is_loading(&self) -> bool {
        self.ingredients.borrow().is_some()
    }

    /// Queues one synchronous operation against the live realm, which settles
    /// what it owes, and answers with what it returned.
    ///
    /// This is the one way into JavaScript. `None` is a realm that is not
    /// live — still loading, or gone — a view that has ended, an operation
    /// that trapped, or a thread that is over; either way nothing of the
    /// operation is observable here.
    ///
    /// The operation *and* the whole epilogue run under one `catch_unwind`
    /// inside [`run_job`], because the bridge erases a panic into "the host
    /// function panicked", a panic on this thread is the view's failure rather
    /// than the group's, and a job runs in the thread's top loop rather than
    /// inside a task that could catch it.
    fn enter<T, O>(self: &Rc<Self>, operation: O) -> impl Future<Output = Option<T>> + use<T, O>
    where
        T: 'static,
        O: FnOnce(&mut MainThreadRuntime, &mut ScriptRuntime) -> T + 'static,
    {
        run_job(self, move |page| page.enter_now(operation))
    }

    /// The body of one entry, as the job runs it.
    fn enter_now<T>(
        self: &Rc<Self>,
        operation: impl FnOnce(&mut MainThreadRuntime, &mut ScriptRuntime) -> T,
    ) -> Option<T> {
        if self.ended() {
            return None;
        }
        // Both borrows are held for the whole entry, a synchronous wait inside
        // the operation included. Nothing else can want them: only a job takes
        // either, and no other job runs until this one returns.
        let js = &mut *self.context.js.borrow_mut();
        let mut realm = self.realm.borrow_mut();
        let runtime = realm.as_deref_mut()?;
        let value = operation(runtime, js);
        self.epilogue(runtime, js);
        Some(value)
    }

    /// Everything one entry into this realm leaves owing.
    ///
    /// The order is the contract:
    ///
    /// 1. **Due timers.** A timer that has come due runs before the commit, so its mutation rides
    ///    the same frame as whatever else this entry changed. A zero-delay timer armed during boot
    ///    therefore fires inside boot's own epilogue and adds no commit of its own.
    /// 2. **The commit**, which is what publishes the frame and the image sources the walk
    ///    discovered.
    /// 3. **The `contentvisibilityautostatechange` deliveries** that commit decided — posted as an
    ///    entry of their own, never run here: see [`Self::post_content_visibility_changes`].
    /// 4. **The `<image>` `load`s and `error`s** this entry settled — posted as an entry of their
    ///    own too: see [`Self::post_image_outcomes`].
    /// 5. **The boot report**, once, so the frame exists before the event that implies it.
    /// 6. **The `BeginFrame` acknowledgement**, for the same reason: a host blocked on the sequence
    ///    number is blocked on that frame.
    /// 7. **The module requests** this entry produced, each spawned as a load of its own, and
    ///    beside them the futures a `.then` asked this realm to settle asynchronously, each spawned
    ///    as a wait of its own.
    /// 8. **The next timer deadline**, republished only when it moved.
    /// 9. **The checkpoint generation**, so the clock task can tell this page's own bumps from a
    ///    sibling's.
    fn epilogue(self: &Rc<Self>, runtime: &mut MainThreadRuntime, js: &mut ScriptRuntime) {
        if self.ended() {
            return;
        }
        #[cfg(test)]
        self.epilogues.set(self.epilogues.get() + 1);
        for failure in runtime.run_due_timers(js) {
            self.outbox.engine_event(EngineEvent::TimerFailed(failure));
        }
        runtime.commit_if_dirty();
        if runtime.has_pending_content_visibility_changes()
            && !self.content_visibility_posted.replace(true)
        {
            self.post_content_visibility_changes();
        }
        if runtime.has_image_outcomes() && !self.image_outcomes_posted.replace(true) {
            self.post_image_outcomes();
        }
        if !self.boot_reported.get() {
            // MTS boot alone: the entry module evaluated and its first flush
            // committed. The BTS Worker's own state is not part of it.
            match runtime.main_module_finished() {
                Ok(false) => {}
                Ok(true) => {
                    self.boot_reported.set(true);
                    self.outbox.engine_event(EngineEvent::ScriptFinished);
                }
                Err(error) => {
                    self.fail(EngineEvent::StartupFailed(error.into_script_error().into()));
                    return;
                }
            }
        }
        if let Some(seq) = self.pending_begin_frame.take() {
            self.outbox.begin_frame_serviced(seq);
        }
        while let Some(url) = runtime.take_module_request() {
            let answer = self
                .outbox
                .request_source(SourceRequest::Module(url.clone()));
            self.spawn(load_module(Rc::clone(self), url, answer));
        }
        for (id, future) in runtime.take_future_settles() {
            self.spawn(settle_future(Rc::clone(self), id, future));
        }
        // Every path that mounts author CSS — the staged sheets
        // `createDocument` mounts, `adoptStyleSheet`, and any rules a card
        // appends — runs inside an entry, so draining here is what covers
        // them all with one call site rather than one per mount.
        for request in runtime.take_font_face_requests() {
            self.spawn(load_font_face(Rc::clone(self), request));
        }
        self.lifetime.arm_deadline(runtime.next_timer_deadline());
        self.lifetime.record_checkpoint(js.checkpoint_generation());
    }

    /// Queues the entry that delivers one commit's
    /// `contentvisibilityautostatechange` events
    /// ([css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event)),
    /// and waits for nothing.
    ///
    /// The spec dispatches the event "by posting a task at the time when the
    /// state change occurs", and this is that post: [`Self::enter`] queues
    /// its job at the call rather than at the first poll of the future it
    /// answers with, so dropping that future leaves one fresh entry behind
    /// every job already queued — with an epilogue of its own, so a handler
    /// that mutates the tree gets its commit. The event is therefore never
    /// delivered inside the entry that committed, and a view that ends in
    /// between delivers nothing.
    ///
    /// **Nothing here enters JavaScript.** The listeners are the engine's own
    /// components, the walk is `dom`'s, and this entry is a job only because
    /// *when* it runs is the whole point. It still goes through
    /// [`Self::enter`] rather than a bare job, because everything that
    /// touches this view's document does: one boundary, one epilogue, one
    /// end latch.
    ///
    /// One entry for the whole batch, in the order the commit queued them:
    /// the queue is drained by this entry rather than by the epilogue that
    /// noticed it, so [`Self::content_visibility_posted`] is what keeps an
    /// entry that runs in between from posting a second one. It is cleared
    /// here, inside the entry; a view that ended before it ran never needs it
    /// again, because nothing is delivered after the end.
    ///
    /// A handler that panics is the view's panic, as a lifecycle callback's
    /// already is: [`run_job`] catches it, [`Self::trapped`] reports the
    /// `ScriptRunError` and the view ends. There is no per-change
    /// `catch_unwind` — a panicking handler leaves the document unspecified,
    /// which is `dom`'s own recorded contract for one, and delivering the
    /// rest of the batch into it would be worse than stopping.
    fn post_content_visibility_changes(self: &Rc<Self>) {
        let page = Rc::clone(self);
        drop(self.enter(move |runtime, _js| {
            // Cleared before the walk, not after: what a handler's own
            // mutation leaves for this entry's epilogue to commit is a batch
            // of its own, and it owes an entry of its own too.
            page.content_visibility_posted.set(false);
            runtime.dispatch_content_visibility_changes();
        }));
    }

    /// Queues the entry that delivers one batch of `<image>` `load` and
    /// `error` events, and waits for nothing.
    ///
    /// The same post as [`Self::post_content_visibility_changes`], because
    /// these events have the same standing: a browser fires an `<img>`'s
    /// `load` from a task, even for a URL the cache already holds, so a
    /// listener never runs inside the entry that bound the `src` and what it
    /// mutates is committed by this entry's own epilogue rather than by that
    /// one's. Posting is also what makes the two producers answerable the
    /// same way at all: the `image` component settles a source from inside
    /// the `__SetAttribute` that wrote it, and dispatching from there would
    /// re-enter the realm in the middle of a host call.
    ///
    /// The epilogue asks after its commit, beside the content-visibility
    /// check, rather than before it. Nothing a commit does settles an image
    /// source — the queue is filled by a bind or by the painting side's
    /// report, both of which happen in the entry's body — so the position
    /// cannot change what is posted; it sits with the other posted delivery
    /// so that "what this entry owes an entry of its own" is one block.
    ///
    /// Unlike that one, this delivery **does** enter JavaScript: `load` and
    /// `error` are script events, and the dispatch is the realm's
    /// `__BobcatDispatchEvent` walk like every other one. A listener that
    /// throws is nonfatal — [`EngineEvent::ListenerFailed`], the standing
    /// every listener here has — and the rest of the batch is still
    /// delivered.
    ///
    /// The latch is cleared at the start of the entry, before the drain: a
    /// handler that writes a `src` this document has already seen settle gets
    /// its outcome at the bind, which queues a new batch that owes an entry
    /// of its own.
    fn post_image_outcomes(self: &Rc<Self>) {
        let page = Rc::clone(self);
        drop(self.enter(move |runtime, js| {
            page.image_outcomes_posted.set(false);
            for failure in runtime.dispatch_image_outcomes(js) {
                page.outbox
                    .engine_event(EngineEvent::ListenerFailed(failure.into_script_error()));
            }
        }));
    }

    /// Reports a panic as the failing view's, and ends the view.
    ///
    /// One report per view whichever path saw the panic first — this thread
    /// catching one inside [`Self::enter`], or the owner reaping a task that
    /// trapped — and it is not gated by [`Self::fail`]'s latch: a view that
    /// already reported a startup failure and then traps still says so.
    fn trapped(&self, payload: &(dyn std::any::Any + Send)) {
        if self.lifetime.report_panic() {
            self.outbox
                .engine_event(EngineEvent::ScriptRunError(panicked(
                    "the Lynx main thread panicked",
                    payload,
                )));
        }
        self.end();
    }

    /// Applies one burst of commands: one entry, one commit, one
    /// acknowledgement.
    ///
    /// Total over every state a page can be in. A live page takes the whole
    /// burst inside one [`Self::enter`]; a loading one writes what it can into
    /// the ingredients its document will be built from, here on the task
    /// rather than in a job, so it is not queued behind a sibling's
    /// synchronous load; a page that has ended drops the burst, `BeginFrame`
    /// included, because the end has already acknowledged the pending one.
    ///
    /// The burst's job reads the view's token as it starts. The token is what
    /// the *embedder* cancelled, and a job start is a wake boundary rather
    /// than the middle of an entry — which matters because the top loop runs
    /// queued jobs back to back, so the owner may not have mirrored a release
    /// onto the latch yet.
    ///
    /// The loop inside the entry checks the latch before each command. It
    /// cannot be truncated from another thread, so one entry is still one
    /// commit; what it stops is the rest of a burst behind a command that
    /// ended the view.
    async fn apply(self: &Rc<Self>, commands: Vec<ToMain>) {
        if self.ended() {
            return;
        }
        // Taken here rather than in the job, because what the seam spawns has
        // to trap the way any other task of this view does, rather than into
        // the `catch_unwind` an entry runs under.
        #[cfg(test)]
        let commands = self.take_test_seams(commands);
        if self.is_loading() {
            self.stage(commands);
            return;
        }
        let page = Rc::clone(self);
        self.enter(move |runtime, js| {
            if page.outbox.is_cancelled() {
                page.end();
                return;
            }
            for command in commands {
                if page.ended() {
                    break;
                }
                page.apply_command(runtime, js, command);
            }
        })
        .await;
    }

    /// Applies one command to a view whose realm exists, including during boot.
    fn apply_command(
        &self,
        runtime: &mut MainThreadRuntime,
        js: &mut ScriptRuntime,
        command: ToMain,
    ) {
        match command {
            ToMain::PageUpdate(update) => {
                // All host lifecycle commands passed LynxView's MTS-boot gate.
                if let Err(error) = runtime.apply_page_update(js, &update) {
                    self.fail(EngineEvent::ScriptRunError(error.into_script_error()));
                }
            }
            ToMain::DispatchEvent {
                target,
                name,
                payload,
            } => {
                // A listener that panics is not fatal to the view: the
                // payload is dropped, the rest of the burst applies, and the
                // epilogue still runs.
                let dispatched = catch_unwind(AssertUnwindSafe(|| {
                    runtime.dispatch_input_event(js, target, name, &payload)
                }));
                if let Ok(Err(error)) = dispatched {
                    self.outbox
                        .engine_event(EngineEvent::ListenerFailed(error.into_script_error()));
                }
            }
            ToMain::Resize {
                width,
                height,
                device_pixel_ratio,
            } => runtime.apply_resize(width, height, device_pixel_ratio),
            ToMain::Vsync(milliseconds) => {
                if let Err(error) = runtime.vsync(js, milliseconds) {
                    self.fail(EngineEvent::ScriptRunError(error.into_script_error()));
                }
            }
            ToMain::BeginFrame { now, seq } => {
                runtime.begin_frame(now);
                let pending = self.pending_begin_frame.get().unwrap_or(0);
                self.pending_begin_frame.set(Some(seq.max(pending)));
            }
            ToMain::Refill { offsets } => runtime.refill_scroll_windows(&offsets),
            ToMain::ImageEvents(events) => runtime.apply_image_events(&events),
            #[cfg(test)]
            ToMain::Probe(probe) => runtime.with_document(probe),
            #[cfg(test)]
            ToMain::Trap(_) => unreachable!("the trap seam is taken before the entry"),
        }
    }

    /// Serves a burst that arrived before the boot module created a document.
    ///
    /// Runs on the task that read the burst rather than in a job, which is
    /// what keeps it immediate: the ingredients are this view's own field and
    /// nothing holds them across a wait, so a sibling view parked on a
    /// synchronous stylesheet cannot delay it.
    ///
    /// What a command can do here is narrow: the two that describe the
    /// document write into the ingredients it will be built from, a
    /// `BeginFrame` is acknowledged at once so an offscreen host is never
    /// blocked by a load, and nothing else has anywhere to go. Dropping a
    /// `Probe` drops the sender it captured, which answers the probing test
    /// `None` rather than leaving it to wait out its deadline.
    fn stage(&self, commands: Vec<ToMain>) {
        let mut acknowledged: Option<u64> = None;
        {
            let mut staged = self.ingredients.borrow_mut();
            let Some(ingredients) = staged.as_mut() else {
                return;
            };
            for command in commands {
                match command {
                    // LynxView rejects lifecycle commands until MTS boot ends.
                    ToMain::PageUpdate(_) => {}
                    ToMain::Resize {
                        width,
                        height,
                        device_pixel_ratio,
                    } => {
                        ingredients.viewport = Viewport::new(width, height)
                            .with_device_pixel_ratio(device_pixel_ratio);
                    }
                    // Kept rather than applied: a report is about a source
                    // some later frame will want, and the document that would
                    // record it does not exist yet.
                    ToMain::ImageEvents(events) => ingredients.pending_image_events.extend(events),
                    ToMain::BeginFrame { seq, .. } => {
                        acknowledged = Some(seq.max(acknowledged.unwrap_or(0)));
                    }
                    ToMain::Vsync(_) | ToMain::DispatchEvent { .. } | ToMain::Refill { .. } => {}
                    #[cfg(test)]
                    ToMain::Probe(_) => {}
                    #[cfg(test)]
                    ToMain::Trap(_) => unreachable!("the trap seam is taken before the entry"),
                }
            }
        }
        if let Some(seq) = acknowledged {
            self.outbox.begin_frame_serviced(seq);
        }
    }

    /// Stages one author sheet, in cascade order. `false` is a page whose
    /// ingredients are spent, which has already ended.
    ///
    /// Task-side, for the reason [`Self::stage`] is.
    fn stage_sheet(&self, sheet: crate::resource::StyleSheetSource) -> bool {
        let mut staged = self.ingredients.borrow_mut();
        let Some(ingredients) = staged.as_mut() else {
            return false;
        };
        ingredients.sheets.push(sheet);
        true
    }

    /// Opens this view's realm and runs its entry, then starts the waits that
    /// only a live realm has.
    ///
    /// A job like every other entry, and the one that does not go through
    /// [`Self::enter`], because the realm it would enter does not exist until
    /// it returns; [`boot_page`] is what queues it. It holds the shared runtime
    /// for the whole stretch, as an entry does — the entry module may adopt a
    /// stylesheet and wait — but takes and stores `realm` under short borrows
    /// either side of it, since what it is building is a local until the last
    /// of them. The checkpoint receiver is created while the runtime borrow is
    /// still held, so no sibling's bump between boot and the clock task's first
    /// poll can be lost.
    ///
    /// It ends by running the first epilogue inline rather than queueing one,
    /// so a boot that finished synchronously is reported in this same stretch.
    ///
    /// The [`RealmStartup`] is everything that realm is opened with, and
    /// opening spends it: `MainThreadRuntime::new` takes the strings it
    /// installs out of it, leaving the entry this then evaluates.
    fn open_realm(self: &Rc<Self>, mut startup: RealmStartup) {
        // A view that has already ended builds no realm and runs no entry:
        // its tasks are about to be reclaimed, and the ingredients go with the
        // page rather than into a document nobody will ever see.
        if self.ended() {
            self.ingredients.borrow_mut().take();
            return;
        }
        let opened = {
            let js = &mut *self.context.js.borrow_mut();
            // Spent before any failure path can report: a page whose realm
            // could not be opened is over, and nothing re-stages what was
            // taken.
            match self.ingredients.borrow_mut().take() {
                None => None,
                Some(ingredients) => self.build_realm(js, *ingredients, &mut startup),
            }
        };
        match opened {
            // Nobody is listening for this view any more, so there is nobody
            // to report to.
            None => {
                self.end();
            }
            Some(Err(error)) => self.fail(EngineEvent::StartupFailed(error)),
            // The latch can have flipped while the entry was inside a
            // synchronous wait: a task ran during it and ended the view, and
            // the owner may already be past its reap and have taken the worker
            // inbox. Tasks spawned behind that reap would never be joined, so
            // none is started and the inbox is left where it is. The realm
            // stays where it is for the owner's release job, which is FIFO
            // behind this one.
            Some(Ok(_)) if self.ended() => {}
            Some(Ok((worker_events, checkpoints))) => {
                *self.worker_events.borrow_mut() = Some(worker_events);
                self.spawn(consume_worker_events(Rc::clone(self)));
                self.spawn(serve_clock(
                    Rc::clone(self),
                    self.lifetime.deadlines(),
                    checkpoints,
                ));
                // The first epilogue, inline: this is already job context and
                // the borrows the stretch above held are released, so a boot
                // that finished synchronously reports in this same stretch
                // rather than a queue trip later. It is what publishes the
                // boot's `ScriptFinished` and spawns the module requests its
                // entry left.
                let _ = self.enter_now(|_, _| ());
            }
        }
    }

    /// Furnishes the realm and evaluates the entry in it, leaving it live.
    ///
    /// `None` is a view released while this ran — the flag is written from the
    /// embedder's own thread, so it can change between two statements here,
    /// and the entry's own synchronous waits are where it usually does.
    #[expect(clippy::type_complexity, reason = "one call site, spelled once")]
    fn build_realm(
        self: &Rc<Self>,
        js: &mut ScriptRuntime,
        ingredients: DocumentIngredients,
        startup: &mut RealmStartup,
    ) -> Option<
        Result<
            (
                mpsc::UnboundedReceiver<WorkerEvent>,
                tokio::sync::watch::Receiver<u64>,
            ),
            LynxViewError,
        >,
    > {
        let (mut runtime, worker_events) = match MainThreadRuntime::new(
            js,
            ingredients,
            self.outbox.clone(),
            &self.context.workers,
            self.lifetime.thread().clone(),
            startup,
        ) {
            Ok(opened) => opened,
            Err(error) => return Some(Err(error.into_script_error().into())),
        };
        if self.outbox.is_cancelled() {
            return None;
        }
        if let Err(error) = runtime.run_main_thread_script(js, &startup.source, &startup.url) {
            if self.outbox.is_cancelled() {
                return None;
            }
            return Some(Err(error.into_script_error().into()));
        }
        let checkpoints = js.checkpoints();
        self.lifetime.record_checkpoint(js.checkpoint_generation());
        *self.realm.borrow_mut() = Some(Box::new(runtime));
        Some(Ok((worker_events, checkpoints)))
    }

    /// This view's whole tail: wait, end, reclaim, dispose, release the realm.
    ///
    /// The wait is the lifetime's — the end, or the next task of the view to
    /// finish. [`Self::end`] after it is what mirrors a cancellation that came
    /// from another thread onto this thread's latch, and what acknowledges a
    /// pending `BeginFrame` so a blocked painter is released. The reap is what
    /// makes this the last owner of the page: a task holds an `Rc` of it until
    /// its future is dropped.
    ///
    /// What follows is jobs, every one of them awaited. That is what keeps the
    /// release from landing on a realm that is under a live JavaScript stack:
    /// a job of this view's that is parked on a synchronous wait is ahead of
    /// the release in the one FIFO, so the release cannot begin until it has
    /// returned.
    async fn run_owner(self: &Rc<Self>) {
        self.lifetime
            .serve(&mut |payload| self.trapped(payload.as_ref()))
            .await;
        self.end();
        self.lifetime
            .reap(&mut |payload| self.trapped(payload.as_ref()))
            .await;
        // The view has ended, but its MTS realm remains alive to exchange
        // disposal messages. No view token cancels its Workers first.
        let events = self.worker_events.borrow_mut().take();
        if let Some(mut events) = events
            && let Err(error) = self.dispose(&mut events).await
        {
            self.outbox
                .engine_event(EngineEvent::ListenerFailed(error.into_script_error()));
        }
        // The realm owns the remaining Worker handles. Releasing it drops
        // their channels naturally; there is no Rust termination sweep.
        self.after_end(|realm, _| *realm = None).await;
    }

    /// The JavaScript disposal exchange, as a run of jobs the owner awaits.
    ///
    /// Two jobs per event rather than three: the first starts the disposal and
    /// answers whether it has already finished, and each delivery answers the
    /// same question about the state it left behind, so the question never
    /// costs a queue trip of its own.
    ///
    /// `Ok(())` is a disposal that finished, one that was never started
    /// because the realm was not live, or a BTS that stopped answering.
    async fn dispose(
        self: &Rc<Self>,
        events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
    ) -> Result<(), super::runtime::MainThreadError> {
        let Some(started) = self
            .after_end(|realm, js| {
                let runtime = realm.as_deref_mut()?;
                Some(
                    runtime
                        .begin_dispose(js)
                        .and_then(|()| runtime.disposal_finished()),
                )
            })
            .await
            .flatten()
        else {
            return Ok(());
        };
        if started? {
            return Ok(());
        }
        loop {
            let Some(WorkerEvent { key, payload }) = events.recv().await else {
                return Ok(());
            };
            let Some((delivered, finished)) = self
                .after_end(move |realm, js| {
                    let runtime = realm.as_deref_mut()?;
                    let delivered = runtime.dispatch_worker_event(js, key, payload);
                    Some((delivered, runtime.disposal_finished()))
                })
                .await
                .flatten()
            else {
                return Ok(());
            };
            if let Err(error) = delivered {
                self.outbox
                    .engine_event(EngineEvent::ListenerFailed(error.into_script_error()));
            }
            if finished? {
                return Ok(());
            }
        }
    }

    /// One job against this view's realm after the view has ended: no latch,
    /// and no epilogue.
    ///
    /// The view is over by the time any of these runs — there is nothing left
    /// to commit, report or re-arm — so it is the disposal exchange and the
    /// release that use it, and nothing else. The realm is still in
    /// [`Self::realm`] throughout, which is what puts the release behind
    /// whatever job is holding it.
    fn after_end<T, O>(self: &Rc<Self>, operation: O) -> impl Future<Output = Option<T>> + use<T, O>
    where
        T: 'static,
        O: FnOnce(&mut Option<Box<MainThreadRuntime>>, &mut ScriptRuntime) -> T + 'static,
    {
        run_job(self, move |page| {
            let js = &mut *page.context.js.borrow_mut();
            Some(operation(&mut page.realm.borrow_mut(), js))
        })
    }

    /// Takes the one command that is not the realm's out of a burst.
    ///
    /// It spawns two tasks of this view — one that panics on its first poll,
    /// and behind it one that records what [`Self::ended`] says when it is
    /// next polled — which is the only way a test can watch a panic reach a
    /// sibling before it reaches the owner.
    #[cfg(test)]
    fn take_test_seams(self: &Rc<Self>, commands: Vec<ToMain>) -> Vec<ToMain> {
        let mut rest = Vec::new();
        for command in commands {
            let ToMain::Trap(recorder) = command else {
                rest.push(command);
                continue;
            };
            self.spawn(async { panic!("a task of the view trapped") });
            let page = Rc::clone(self);
            self.spawn(async move {
                let _ = recorder.send(page.ended());
            });
        }
        rest
    }

    /// How many tasks this view still has, for a test that pins an end
    /// reaching every one of them.
    #[cfg(test)]
    pub(super) fn task_count(&self) -> usize {
        self.lifetime.task_count()
    }

    /// How many times the epilogue has run.
    #[cfg(test)]
    pub(super) fn epilogue_count(&self) -> u64 {
        self.epilogues.get()
    }

    /// Leaves a `BeginFrame` applied and unacknowledged, which is the state a
    /// painter blocked on that sequence number leaves a view in.
    #[cfg(test)]
    pub(super) fn arm_begin_frame_for_test(&self, seq: u64) {
        self.pending_begin_frame.set(Some(seq));
    }

    /// This view's earliest armed timer, for a test that pins the end
    /// withdrawing it.
    #[cfg(test)]
    pub(super) fn armed_deadline(&self) -> Option<ClockInstant> {
        self.lifetime.armed_deadline()
    }
}

/// What this view's clock task, its unwind guard and its jobs reach it
/// through.
impl Settles for Page {
    fn lifetime(&self) -> &Lifetime {
        &self.lifetime
    }

    /// The epilogue alone, for a wake that carries no operation of its own —
    /// a timer deadline, or a sibling's checkpoint.
    fn settle(owner: &Rc<Self>) -> impl Future<Output = Option<()>> {
        owner.enter(|_, _| ())
    }

    fn end(owner: &Rc<Self>) {
        owner.end();
    }

    fn trapped(owner: &Rc<Self>, payload: &(dyn std::any::Any + Send)) {
        owner.trapped(payload);
    }
}

/// One view's whole life on this thread: build the page, spawn its waits,
/// wait for the end, reclaim.
///
/// Task lifetime and nothing else. No command, no source, no timer and no
/// frame is decided here.
pub(super) async fn serve_view(context: Rc<GroupContext>, view: AttachedView, outbox: ViewOutbox) {
    let AttachedView {
        viewport,
        sources,
        native_modules,
        commands,
        cancel,
    } = view;
    // On every exit path, ordinary or trapped: a host still holding one of
    // this view's source completions must see it cancelled without waiting
    // for a turn of its own. Workers retain their independent lifetimes.
    let _cancel = cancel.clone().drop_guard();
    let ViewSources {
        config,
        fonts,
        default_font_family,
        style_sheets,
        entry,
        background_entry,
        init_data,
        initial_processor,
        global_props,
    } = sources;
    // The fonts first, because a view whose containers cannot serve the family
    // it named will never render and there is nothing worth fetching for it.
    let text_context = match super::stage_text_context(fonts, default_font_family.as_deref()) {
        Ok(text_context) => text_context,
        Err(error) => {
            outbox.engine_event(EngineEvent::StartupFailed(error));
            return;
        }
    };
    let ingredients = DocumentIngredients {
        viewport,
        config,
        text_context,
        sheets: Vec::with_capacity(style_sheets.len()),
        style_pool: context.style_pool.clone(),
        pending_image_events: Vec::new(),
    };
    let page = Page::new(context, outbox, ingredients, cancel);
    page.spawn(consume_commands(Rc::clone(&page), commands));
    page.spawn(boot_page(
        Rc::clone(&page),
        BootSources {
            style_sheets,
            entry,
            background_entry,
            init_data,
            initial_processor,
            global_props,
            native_modules,
        },
    ));
    page.run_owner().await;
}

/// The one ordered consumer of this view's command stream.
///
/// A burst rather than a command: the rest of what is already queued goes
/// with the first, so a host's whole round of input is one commit and one
/// acknowledgement. Bounded by the snapshot the count was taken from, so a
/// producer faster than this task cannot starve the epilogue.
///
/// **A burst queued behind the embedder's release is discarded.** The token is
/// read once per burst, below, and a command whose wake is served after the
/// release ends the view instead of being applied. Not because of wake order:
/// a command already queued wakes this task *before* the release wakes the
/// owner, so on that path this check is the only thing between the burst and
/// the realm. It is a decision rather than an accident — the embedder released
/// the view and can observe nothing of it afterwards. A fatal event's cancel
/// in `LynxView::pump`, which leaves the channel open, takes the same path.
///
/// A painter that was watching is unaffected either way. On the release path
/// the view's seat goes with it — the host's resource system among its fields —
/// so `Painter::poll_link` can no longer upgrade it and does not adopt a commit
/// whose pixels it is not already holding; on the fatal-`pump` path that seat
/// is still alive, and this check is what keeps a frame from being published
/// after the cancel at all.
///
/// What is left over is a burst that was already inside [`Page::apply`] when
/// the cancel landed: it finishes, the way synchronous JavaScript already
/// executing does. Only this thread writes the latch an entry reads.
async fn consume_commands(page: Rc<Page>, mut commands: mpsc::UnboundedReceiver<ToMain>) {
    while let Some(first) = commands.recv().await {
        // The embedder released the view between this command's send and this
        // poll. The token is read here and never inside an entry: a wake
        // boundary is not a synchronous stretch another thread could flip it
        // in the middle of.
        if page.outbox.is_cancelled() {
            page.end();
            return;
        }
        let queued = commands.len();
        let rest = std::iter::from_fn(|| commands.try_recv().ok()).take(queued);
        // Collected rather than handed over as an iterator, because a job owns
        // what it runs on: one allocation per burst, not per command.
        let burst: Vec<ToMain> = std::iter::once(first).chain(rest).collect();
        // Awaited, so the next burst is read only once this one has been
        // applied — which is what makes what arrives meanwhile one later
        // burst rather than a queue of entries.
        page.apply(burst).await;
    }
    // The embedder released this view. Everything it owns goes with the
    // tasks the owner is about to reclaim.
    page.end();
}

/// The page's boot future: every author sheet in cascade order, then the
/// entry, then the realm.
///
/// A sheet that arrives after the entry has run would restyle a document the
/// card has already built, so they are requested one at a time and in order.
/// They are staged rather than mounted, because there is no document yet —
/// `createDocument` mounts them in this order, and it runs before the entry.
async fn boot_page(page: Rc<Page>, sources: BootSources) {
    let BootSources {
        style_sheets,
        entry,
        background_entry,
        init_data,
        initial_processor,
        global_props,
        native_modules,
    } = sources;
    for url in style_sheets {
        if page.outbox.is_cancelled() {
            page.end();
            return;
        }
        match request_source(&page.outbox, SourceRequest::StyleSheet(url)).await {
            Ok(LoadedSource::StyleSheet(sheet)) => {
                if !page.stage_sheet(sheet) {
                    return;
                }
            }
            Ok(LoadedSource::Entry { .. }) => {
                page.fail(EngineEvent::StartupFailed(mismatched_source(
                    "a stylesheet request",
                    "a script",
                )));
                return;
            }
            Ok(LoadedSource::Font(_)) => {
                page.fail(EngineEvent::StartupFailed(mismatched_source(
                    "a stylesheet request",
                    "a font",
                )));
                return;
            }
            Err(error) => {
                page.fail(EngineEvent::StartupFailed(error));
                return;
            }
        }
    }
    if page.outbox.is_cancelled() {
        page.end();
        return;
    }
    let (source, url) = match request_source(&page.outbox, SourceRequest::Entry(entry)).await {
        Ok(LoadedSource::Entry { source, url }) => (source, url),
        Ok(LoadedSource::Font(_)) => {
            page.fail(EngineEvent::StartupFailed(mismatched_source(
                "an entry request",
                "a font",
            )));
            return;
        }
        Ok(LoadedSource::StyleSheet(_)) => {
            page.fail(EngineEvent::StartupFailed(mismatched_source(
                "an entry request",
                "a stylesheet",
            )));
            return;
        }
        Err(error) => {
            page.fail(EngineEvent::StartupFailed(error));
            return;
        }
    };
    if page.outbox.is_cancelled() {
        page.end();
        return;
    }
    let startup = RealmStartup {
        source,
        url,
        background_entry,
        initial_processor,
        init_data,
        global_props,
        native_modules,
    };
    run_job(&page, move |page| {
        page.open_realm(startup);
        Some(())
    })
    .await;
}

/// One resource load an import produced.
///
/// An import that cannot be completed is fatal to whatever was awaiting it,
/// so the failure ends the view — reported as a startup failure while boot is
/// still outstanding, and as a run failure once it has been reported.
async fn load_module(page: Rc<Page>, url: String, answer: SourceAnswer) {
    let source = answer
        .await
        .unwrap_or_else(|_| Err(unanswered_source().into()));
    let completing = Rc::clone(&page);
    page.enter(move |runtime, js| {
        if let Err(error) = runtime.complete_module(js, &url, source) {
            let error = error.into_script_error();
            completing.fail(if completing.boot_reported.get() {
                EngineEvent::ScriptRunError(error)
            } else {
                EngineEvent::StartupFailed(error.into())
            });
        }
    })
    .await;
}

/// One future a `.then` asked this realm to settle.
///
/// Its shape is [`load_module`]'s, and so is what a failure costs: nothing in
/// the realm is waiting for a settle that cannot be delivered, but the failure
/// is the realm refusing a call rather than the operation failing — which is
/// fatal to whatever the Promise was holding up. Reported as a startup failure
/// while boot is still outstanding, and as a run failure once it has been
/// reported.
async fn settle_future(page: Rc<Page>, id: u32, future: crate::future::HostFuture) {
    let outcome = future.await;
    let settling = Rc::clone(&page);
    page.enter(move |runtime, js| {
        if let Err(error) = runtime.deliver_future(js, id, outcome) {
            let error = error.into_script_error();
            settling.fail(if settling.boot_reported.get() {
                EngineEvent::ScriptRunError(error)
            } else {
                EngineEvent::StartupFailed(error.into())
            });
        }
    })
    .await;
}

/// One `@font-face` rule's sources, tried in author order until one loads.
///
/// Unlike a module, nothing is waiting for a declared face, so no failure
/// here ends the view: a rule whose every source fails leaves the runs that
/// name its family on the fallback they already had, which is what a browser
/// does for a face it could not fetch. It is dropped without a word — this
/// workspace has no logging facade and this would be its only use, and an
/// [`EngineEvent`] is the wrong seam because every variant of it is either
/// fatal or something a realm said.
///
/// `local()` is skipped: naming a face already installed on the platform needs
/// a platform font enumerator this engine does not have, and the native
/// Harmony loader refuses it too.
async fn load_font_face(page: Rc<Page>, request: dom::FontFaceRequest) {
    let dom::FontFaceRequest { family, sources } = request;
    let mut loaded = None;
    for source in sources {
        let dom::FontFaceSource::Url(url) = source else {
            continue;
        };
        if page.ended() {
            return;
        }
        // A face the fetcher answered with a script or a stylesheet is a
        // failed source like any other: try the next one.
        if let Ok(LoadedSource::Font(blob)) =
            request_source(&page.outbox, SourceRequest::Font { url }).await
        {
            loaded = Some(blob);
            break;
        }
    }
    let Some(blob) = loaded else {
        return;
    };
    // The entry's own epilogue commits: registering a face invalidates the
    // layout of every run that names it.
    page.enter(move |runtime, _| runtime.register_font_face(&family, blob))
        .await;
}

/// The one ordered consumer of everything this view's workers say.
///
/// The channel's senders are the realm's `WorkerOwner` and this view's worker
/// tasks, so it closes only once the realm is gone — which is to say, with
/// the view.
async fn consume_worker_events(page: Rc<Page>) {
    loop {
        let event = tokio::select! {
            biased;
            () = page.lifetime.token().cancelled() => return,
            event = poll_fn(|cx| {
                page.worker_events
                    .borrow_mut()
                    .as_mut()
                    .expect("the live realm installed its Worker inbox")
                    .poll_recv(cx)
            }) => event,
        };
        let Some(WorkerEvent { key, payload }) = event else {
            return;
        };
        let delivered = page
            .enter(move |runtime, js| runtime.dispatch_worker_event(js, key, payload))
            .await;
        // A configured BTS entry can reject the boot promise in this checkpoint.
        // The epilogue reports that as StartupFailed and ends the view; only an
        // error that leaves the view running is a listener failure.
        if let Some(Err(error)) = delivered
            && !page.ended()
        {
            page.outbox
                .engine_event(EngineEvent::ListenerFailed(error.into_script_error()));
        }
    }
}

/// Asks the host for one source and waits for it. A fetcher that dropped the
/// request without answering is a failed load, not a wait forever.
async fn request_source(
    outbox: &ViewOutbox,
    request: SourceRequest,
) -> Result<LoadedSource, LynxViewError> {
    outbox
        .request_source(request)
        .await
        .unwrap_or_else(|_| Err(unanswered_source().into()))
}

fn mismatched_source(request: &str, answer: &str) -> LynxViewError {
    let mut error = unanswered_source();
    error.message = std::sync::Arc::from(format!("the fetcher answered {request} with {answer}"));
    error.into()
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod page_tests;

#[cfg(test)]
#[path = "font_face_tests.rs"]
mod font_face_tests;
