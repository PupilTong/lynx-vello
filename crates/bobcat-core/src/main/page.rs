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
//! - [`consume_metrics`], which settles the page once per change of the painter's metrics, so a
//!   resize with no JavaScript behind it still commits;
//! - one [`load_entry`] for its MTS entry, entering the realm when its answer arrives;
//! - one [`owner::load_module`] per resource load an import produced, and one
//!   [`owner::settle_future`] per host-backed `Future` a `.then` asked this realm to settle, each
//!   spawned by the driver's epilogue;
//! - one [`load_font_face`] per `@font-face` rule a mounted sheet declared;
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
//! Every one of those tasks reaches the realm through [`owner::enter`], the
//! driver a view shares with every worker, which queues one job and answers
//! with what it returned. The job runs one synchronous operation and then the
//! driver's epilogue, which settles what that operation left owing: due
//! timers, the commit, the boot report, the `BeginFrame` acknowledgement, the
//! module requests and future settles the entry produced, the `@font-face`
//! loads, the next timer deadline. [`Settles::settle`] is the epilogue alone,
//! for a wake that carries no operation of its own.
//! [`Page::open_realm`] is a job too, and the only one that does not go
//! through `enter`, because the realm it would enter does not exist until it
//! returns; the disposal exchange the page runs before [`owner::run_owner`]
//! releases its realm is the other exception, running after the view has
//! ended and so past the latch and the epilogue.
//!
//! The epilogue's steps, and their order, are the driver's
//! ([`crate::realm::owner`]). What the driver is told about a page is the
//! page's [`RealmOwner`] impl: where the realm, the runtime and the host are,
//! that its reports are engine events, the epilogue steps only a page has —
//! the commit and the deliveries it posts, `ScriptFinished`, the `BeginFrame`
//! acknowledgement, the `@font-face` loads — and the two things only a page
//! owes — the `BeginFrame` acknowledgement at the end, and the JavaScript
//! disposal before the release.
//!
//! What a failure is reported as, and whether it ends the view, is the MTS
//! table in [`policy`], read by the scene the failure happened in: every
//! report here goes through [`policy::report`], except the disposal's, which
//! is the table's Disposal row, and an entry the fetcher could not load, which
//! is the Open row over the fetcher's own error.
//!
//! Nothing of this view's is served outside a job. [`Page::open_realm`] is the
//! *first* job of every view, queued before any of those tasks is spawned, so
//! a command that arrived before the realm existed is a job queued behind it
//! rather than something a task has to answer on its own.
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
//! document's ingredients are held by the realm from the moment it opens, and
//! the first job of the view is what spends them.
//!
//! # Waits
//!
//! After this module every `select!` in this crate is one of six kinds, and
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
//! - **a worker's message consumer** — one `consume_messages` per worker, waiting on what is
//!   posted, termination included, versus the worker's script while it is outstanding, versus the
//!   worker's root module finishing until it has;
//! - **the painter's metrics** — one [`consume_metrics`] per view, waiting on the end versus the
//!   next value the view's seat publishes.
//!
//! How many there are is the group's shape rather than a constant: one of the
//! first two kinds per engine thread, one of the third and one of the sixth
//! per live view, one of the third per live worker, one of the fourth per
//! live realm, one of the fifth per live worker.
//! `link.rs`'s `block_on_deadline` is a hand-rolled poll loop rather than a
//! select, and the only one left. The synchronous host members are a wait of
//! their own shape — this view's token against the answer — parked on inside
//! a job through [`JsThread::wait`](crate::jobs::JsThread): stylesheet
//! adoption, [`crate::future`]'s `waitFuture`, which adds an optional
//! deadline behind the token, and `__FlushElementTree`, which waits for each
//! listed author sheet not answered yet and then, before a painter has bound,
//! on that same metrics watch. Those two waits inside boot's own flush are
//! the only things boot waits on.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::rc::Rc;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::quickjs::{ScriptRuntime, SharedRuntime};
use super::runtime::{DocumentIngredients, MainThreadRuntime, RealmStartup};
use super::{AttachedView, GroupContext};
use crate::background::WorkerEvent;
#[cfg(test)]
use crate::clock::ClockInstant;
use crate::lifetime::{Lifetime, Settles, run_job, serve_clock};
use crate::link::{HostOutbox, ToMain, ViewOutbox};
use crate::realm::RealmCore;
use crate::realm::owner::{self, RealmOwner};
use crate::realm::policy::{self, Row, Scene};
use crate::resource::{LoadedSource, SourceRequest};
use crate::script::ScriptError;
use crate::threads::platform_script_error;
use crate::view::{
    EngineEvent, LynxViewError, StartupSource, StartupSources, ViewSources, Viewport,
};

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
    /// What this view's realm asks the host through, over the view's own
    /// token: the module requests the epilogue makes.
    host: HostOutbox,
    /// This view's realm, from the moment [`Page::open_realm`] returns: `None`
    /// only before that first job has run, and again once the realm could not
    /// be opened or the owner has released it. It is the only thing that
    /// decides what a command can do to this page.
    ///
    /// Because that job is the view's first, nothing else of this view's can
    /// observe the `None`: a burst that arrived earlier is a job queued behind
    /// it, and a realm that failed to open has already ended the view.
    ///
    /// Read and written only inside a job, which holds this borrow for its
    /// whole length, its own synchronous wait included.
    ///
    /// Boxed because this is one field of a page every task of the view holds,
    /// and a page is live for exactly one phase of its life.
    realm: RefCell<Option<Box<MainThreadRuntime>>>,
    /// The painter's metrics, as the view's seat publishes them: `None` until
    /// one binds.
    ///
    /// Held here only to hand a clone to the realm as it opens — the document
    /// is what reads it, and [`consume_metrics`] owns a clone of its own so
    /// that marking a value seen on one receiver says nothing about the
    /// other.
    metrics: watch::Receiver<Option<Viewport>>,
    /// The same inbox serves ordinary events and the final JS disposal RPC.
    /// The owner takes it only after the ordinary consumer has been reaped.
    worker_events: RefCell<Option<mpsc::UnboundedReceiver<WorkerEvent>>>,
    /// The newest `BeginFrame` sequence applied and not yet acknowledged.
    ///
    /// Acknowledged in the epilogue rather than where it is applied: a host
    /// blocked on this number is waiting for the frame it implies, which the
    /// epilogue's commit is what publishes. The end acknowledges it too,
    /// because that frame will then never come.
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
    /// reads, the latches its `StartupFailed` and its `Panicked` go through,
    /// and the two numbers this realm's clock task waits on — the deadline it
    /// armed and the generation its own last entry recorded.
    lifetime: Lifetime,
}

impl Page {
    /// A page whose realm has not been opened yet, over the token that ends
    /// it.
    fn new(
        context: Rc<GroupContext>,
        outbox: ViewOutbox,
        metrics: watch::Receiver<Option<Viewport>>,
        token: CancellationToken,
    ) -> Rc<Self> {
        let lifetime = Lifetime::new(token, context.thread.clone());
        let host = outbox.host_outbox(outbox.token().clone());
        Rc::new(Self {
            context,
            outbox,
            host,
            realm: RefCell::new(None),
            metrics,
            worker_events: RefCell::new(None),
            pending_begin_frame: Cell::new(None),
            boot_reported: Cell::new(false),
            content_visibility_posted: Cell::new(false),
            image_outcomes_posted: Cell::new(false),
            lifetime,
        })
    }

    fn ended(&self) -> bool {
        self.lifetime.ended()
    }

    /// Acknowledges the newest `BeginFrame` applied and not yet acknowledged,
    /// if there is one: in the epilogue once the frame it implies is
    /// committed, and at the end, because that frame will then never come.
    fn acknowledge_begin_frame(&self) {
        if let Some(seq) = self.pending_begin_frame.take() {
            self.outbox.begin_frame_serviced(seq);
        }
    }

    /// Queues the entry that delivers one commit's
    /// `contentvisibilityautostatechange` events
    /// ([css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event)),
    /// and waits for nothing.
    ///
    /// The spec dispatches the event "by posting a task at the time when the
    /// state change occurs", and this is that post: [`owner::enter`] queues
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
    /// [`owner::enter`] rather than a bare job, because everything that
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
    /// already is: [`run_job`] catches it, [`owner::trapped`] reports the
    /// `Panicked` and the view ends. There is no per-change
    /// `catch_unwind` — a panicking handler leaves the document unspecified,
    /// which is `dom`'s own recorded contract for one, and delivering the
    /// rest of the batch into it would be worse than stopping.
    fn post_content_visibility_changes(self: &Rc<Self>) {
        let page = Rc::clone(self);
        drop(owner::enter(self, move |runtime, _js| {
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
        drop(owner::enter(self, move |runtime, js| {
            page.image_outcomes_posted.set(false);
            for failure in runtime.dispatch_image_outcomes(js) {
                policy::report(&page, Scene::Listener, failure.into_script_error());
            }
        }));
    }

    /// Applies one burst of commands: one entry, one commit, one
    /// acknowledgement.
    ///
    /// Total over every state a page can be in. A live page takes the whole
    /// burst inside one [`owner::enter`]; a page that has ended drops the
    /// burst, `BeginFrame` included, because the end has already acknowledged
    /// the pending one. There is no third state a task can observe: the job
    /// that opens the realm is the first of the view, so a burst that arrived
    /// before it is queued behind it and finds a document.
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
        let page = Rc::clone(self);
        owner::enter(self, move |runtime, js| {
            if page.outbox.is_cancelled() {
                owner::end(&page);
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
    ///
    /// A command whose script fails is reported and the burst goes on, boot
    /// finished or not: the failure is the app's, and the realm is still
    /// there for the next command. What it is reported as is the MTS table's
    /// row for the kind of command — a page update and a native module's
    /// answer are host calls, an input event a listener's, a vsync a frame's.
    /// A panic is the view's, as anywhere else in an entry: [`run_job`]
    /// catches it and [`owner::trapped`] reports it.
    fn apply_command(
        self: &Rc<Self>,
        runtime: &mut MainThreadRuntime,
        js: &mut ScriptRuntime,
        command: ToMain,
    ) {
        match command {
            ToMain::PageUpdate(update) => {
                // All host lifecycle commands passed LynxView's MTS-boot gate.
                if let Err(error) = runtime.apply_page_update(js, &update) {
                    policy::report(self, Scene::HostCall, error.into_script_error());
                }
            }
            ToMain::DispatchEvent {
                target,
                name,
                payload,
            } => {
                if let Err(error) = runtime.dispatch_input_event(js, target, name, &payload) {
                    policy::report(self, Scene::Listener, error.into_script_error());
                }
            }
            ToMain::Vsync(milliseconds) => {
                if let Err(error) = runtime.vsync(js, milliseconds) {
                    policy::report(self, Scene::Frame, error.into_script_error());
                }
            }
            ToMain::BeginFrame { now, seq } => {
                runtime.begin_frame(now);
                let pending = self.pending_begin_frame.get().unwrap_or(0);
                self.pending_begin_frame.set(Some(seq.max(pending)));
            }
            ToMain::Refill { offsets } => runtime.refill_scroll_windows(&offsets),
            ToMain::ModuleCallback {
                call,
                index,
                arguments,
            } => {
                if let Err(error) =
                    runtime.deliver_module_callback(js, call, index, arguments.as_deref())
                {
                    policy::report(self, Scene::HostCall, error.into_script_error());
                }
            }
            ToMain::ImageEvents(events) => runtime.apply_image_events(&events),
            #[cfg(test)]
            ToMain::Probe(probe) => runtime.with_document(probe),
            #[cfg(test)]
            ToMain::Trap(_) => unreachable!("the trap seam is taken before the entry"),
        }
    }

    /// Opens this view's realm and runs its entry, then starts the waits that
    /// only a live realm has.
    ///
    /// **The first job of every view**, queued by [`serve_view`] before any of
    /// that view's tasks is spawned and before anything has been fetched: the
    /// boot module creates the document as its first statement and imports
    /// the entry, which [`load_entry`] completes when its answer arrives; the
    /// author sheets are the realm's, settled by boot's first flush. So a
    /// command or the entry that arrived before the realm existed is a job
    /// queued behind this one, and finds a document.
    ///
    /// A job like every other entry, and the one that does not go through
    /// [`owner::enter`], because the realm it would enter does not exist until
    /// it returns. It holds the shared runtime
    /// for the whole stretch, as an entry does — an entry already completed
    /// runs inside it, and may adopt a stylesheet and wait — but takes and
    /// stores `realm` under short borrows
    /// either side of it, since what it is building is a local until the last
    /// of them. The checkpoint receiver is created while the runtime borrow is
    /// still held, so no sibling's bump between boot and the clock task's first
    /// poll can be lost.
    ///
    /// It ends by running the first epilogue inline rather than queueing one,
    /// so a boot that finished synchronously is reported in this same stretch.
    ///
    /// The [`RealmStartup`] is everything that realm is opened with, and
    /// opening spends it. A group whose runtime could not be built opens no
    /// realm: the view fails its startup with the runtime's error, and the
    /// startup — the answers to its listed sheets included — is dropped.
    fn open_realm(self: &Rc<Self>, ingredients: DocumentIngredients, startup: RealmStartup) {
        // A view that has already ended builds no realm and runs no entry:
        // its tasks are about to be reclaimed, and the ingredients go with the
        // page rather than into a document nobody will ever see.
        if self.ended() {
            return;
        }
        let opened = {
            let mut js = self.context.js.borrow_mut();
            match js.as_mut() {
                // The runtime failed once, for every view that will ever
                // attach to this group. Each hears the same reason.
                Err(error) => Some(Err(error.clone())),
                Ok(js) => self.build_realm(js, ingredients, startup),
            }
        };
        match opened {
            // Nobody is listening for this view any more, so there is nobody
            // to report to.
            None => owner::end(self),
            Some(Err(error)) => policy::report(self, Scene::Open, error),
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
                owner::spawn(self, consume_worker_events(Rc::clone(self)));
                owner::spawn(
                    self,
                    serve_clock(Rc::clone(self), self.lifetime.deadlines(), checkpoints),
                );
                // The first epilogue, inline: this is already job context and
                // the borrows the stretch above held are released, so a boot
                // that finished synchronously reports in this same stretch
                // rather than a queue trip later. It is what publishes the
                // boot's `ScriptFinished` and spawns the module requests its
                // entry left.
                let _ = owner::enter_now(self, |_, _| ());
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
        startup: RealmStartup,
    ) -> Option<
        Result<
            (
                mpsc::UnboundedReceiver<WorkerEvent>,
                tokio::sync::watch::Receiver<u64>,
            ),
            ScriptError,
        >,
    > {
        let (mut runtime, worker_events) = match MainThreadRuntime::new(
            js,
            ingredients,
            self.metrics.clone(),
            self.outbox.clone(),
            &self.context.workers,
            self.lifetime.thread().clone(),
            startup,
        ) {
            Ok(opened) => opened,
            Err(error) => return Some(Err(error.into_script_error())),
        };
        if self.outbox.is_cancelled() {
            return None;
        }
        if let Err(error) = runtime.run_boot_module(js) {
            if self.outbox.is_cancelled() {
                return None;
            }
            return Some(Err(error.into_script_error()));
        }
        let checkpoints = js.checkpoints();
        self.lifetime.record_checkpoint(js.checkpoint_generation());
        *self.realm.borrow_mut() = Some(Box::new(runtime));
        Some(Ok((worker_events, checkpoints)))
    }

    /// The JavaScript disposal exchange, as a run of [`owner::after_end`] jobs
    /// the owner awaits.
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
        let Some(started) = owner::after_end(self, |realm, js| {
            let runtime = realm.as_deref_mut()?;
            Some(
                runtime
                    .begin_dispose(js)
                    .and_then(|()| runtime.disposal_finished()),
            )
        })
        .await
        .flatten() else {
            return Ok(());
        };
        if started? {
            return Ok(());
        }
        loop {
            let Some(WorkerEvent { key, payload }) = events.recv().await else {
                return Ok(());
            };
            let Some((delivered, finished)) = owner::after_end(self, move |realm, js| {
                let runtime = realm.as_deref_mut()?;
                let delivered = runtime.dispatch_worker_event(js, key, payload);
                Some((delivered, runtime.disposal_finished()))
            })
            .await
            .flatten() else {
                return Ok(());
            };
            if let Err(error) = delivered {
                self.send(policy::main_thread_disposal().event_for(error.into_script_error()));
            }
            if finished? {
                return Ok(());
            }
        }
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
            owner::spawn(self, async { panic!("a task of the view trapped") });
            let page = Rc::clone(self);
            owner::spawn(self, async move {
                let _ = recorder.send(page.ended());
            });
        }
        rest
    }

    /// [`owner::spawn`], for a test that starts a task of this view's itself.
    #[cfg(test)]
    fn spawn(self: &Rc<Self>, future: impl std::future::Future<Output = ()> + 'static) {
        owner::spawn(self, future);
    }

    /// [`owner::terminal`], for a test that fails this view itself.
    #[cfg(test)]
    fn fail(self: &Rc<Self>, event: EngineEvent) {
        owner::terminal(self, event);
    }

    /// [`owner::run_owner`], for a test that runs this view's tail itself.
    #[cfg(test)]
    async fn run_owner(self: &Rc<Self>) {
        owner::run_owner(self).await;
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
        self.lifetime.epilogue_count()
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

/// What the driver in [`owner`] is told about a view: where its realm, its
/// runtime and its host are, that its reports are engine events, and what a
/// page adds to the epilogue, the end and the release — the commit and the
/// deliveries it posts, the boot report, the `BeginFrame` acknowledgement,
/// the `@font-face` loads, and the JavaScript disposal before the release.
impl RealmOwner for Page {
    type Realm = MainThreadRuntime;
    type Event = EngineEvent;

    /// A rejection of boot's own module is the engine's code failing, not
    /// the app's: boot catches what the entry's evaluation throws (see
    /// [`MainThreadRuntime::run_boot_module`]), so what rejects it is
    /// `bobcat:runtime`, the document's construction, connecting the BTS, or
    /// boot's own flush — a listed sheet that could not be loaded included.
    /// It fails the view's startup.
    const BOOT_REJECTION: Option<(Scene, Option<&'static str>)> =
        Some((Scene::Open, Some("booting the MTS entry")));

    fn lifetime(&self) -> &Lifetime {
        &self.lifetime
    }

    fn runtime(&self) -> &SharedRuntime {
        &self.context.js
    }

    fn realm(&self) -> &RefCell<Option<Box<MainThreadRuntime>>> {
        &self.realm
    }

    fn core(runtime: &mut MainThreadRuntime) -> &mut RealmCore {
        runtime.core()
    }

    fn host(&self) -> &HostOutbox {
        &self.host
    }

    fn send(&self, event: EngineEvent) {
        self.outbox.engine_event(event);
    }

    /// The MTS table: what could not be made ready fails the view's startup
    /// and ends it, and everything else is the app's failure, reported while
    /// the view goes on.
    fn row(scene: Scene) -> Row<EngineEvent> {
        policy::main_thread(scene)
    }

    fn panic_row() -> Row<EngineEvent> {
        policy::main_thread_panic()
    }

    /// The realm's own due timers, and the collection the removals they made
    /// may have made due. A zero-delay timer armed during boot therefore
    /// fires inside boot's own epilogue and adds no commit of its own.
    fn run_due_timers(runtime: &mut MainThreadRuntime, js: &mut ScriptRuntime) -> Vec<ScriptError> {
        runtime.run_due_timers(js)
    }

    /// The commit, which is what publishes the frame and the image sources
    /// the walk discovered — skipped while any listed author sheet is
    /// outstanding, because the first `__FlushElementTree` is what waits for
    /// them and a commit without them would publish an unstyled frame. Then
    /// the `contentvisibilityautostatechange` deliveries that commit decided
    /// and the `<image>` `load`s and `error`s this entry settled, each posted
    /// as an entry of its own and never run here: see
    /// [`Page::post_content_visibility_changes`] and
    /// [`Page::post_image_outcomes`].
    fn after_timers(page: &Rc<Self>, runtime: &mut MainThreadRuntime) {
        runtime.commit_if_dirty();
        if runtime.has_pending_content_visibility_changes()
            && !page.content_visibility_posted.replace(true)
        {
            page.post_content_visibility_changes();
        }
        if runtime.has_image_outcomes() && !page.image_outcomes_posted.replace(true) {
            page.post_image_outcomes();
        }
    }

    fn booted(&self) -> bool {
        self.boot_reported.get()
    }

    fn mark_booted(&self) {
        self.boot_reported.set(true);
    }

    /// MTS boot alone: the entry module evaluated and boot's first flush
    /// committed. The BTS Worker's own state is not part of it.
    fn on_booted(&self) {
        self.outbox.engine_event(EngineEvent::ScriptFinished);
    }

    /// The `BeginFrame` acknowledgement, after the commit and the boot
    /// report: a host blocked on the sequence number is blocked on the frame
    /// it implies.
    fn after_boot(&self) {
        self.acknowledge_begin_frame();
    }

    /// The entry's own request is answered by [`load_entry`], a task of this
    /// view since it was served, from the answer `create_lynx_view` already
    /// asked for.
    fn entry_name(&self, runtime: &MainThreadRuntime) -> Option<String> {
        runtime.entry_module_name().ok()
    }

    /// One `@font-face` load per rule this entry's sheets declared. Every
    /// path that mounts author CSS — the listed sheets the first
    /// `__FlushElementTree` mounts, `adoptStyleSheet`, and any rules a card
    /// appends — runs inside an entry, so draining here is what covers them
    /// all with one call site rather than one per mount.
    fn after_settles(page: &Rc<Self>, runtime: &mut MainThreadRuntime) {
        for request in runtime.take_font_face_requests() {
            owner::spawn(page, load_font_face(Rc::clone(page), request));
        }
    }

    /// A painter blocked in `wait_begin_frame` is released rather than timed
    /// out: the frame it was waiting for will never come. The deadline the
    /// realm had armed is withdrawn by the lifetime's own end.
    fn on_end(&self) {
        self.acknowledge_begin_frame();
    }

    /// The JavaScript disposal: the view has ended, but its MTS realm remains
    /// alive to exchange disposal messages with the BTS over the worker inbox,
    /// which is taken only now that its ordinary consumer has been reaped. No
    /// view token cancels its Workers first.
    ///
    /// The release that follows drops the realm, and with it the remaining
    /// Worker handles, whose channels close with them: there is no Rust
    /// termination sweep.
    async fn before_release(page: &Rc<Self>) {
        let events = page.worker_events.borrow_mut().take();
        if let Some(mut events) = events
            && let Err(error) = page.dispose(&mut events).await
        {
            page.send(policy::main_thread_disposal().event_for(error.into_script_error()));
        }
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
        text_context,
        startup,
        native_modules,
        commands,
        metrics,
        cancel,
    } = view;
    // On every exit path, ordinary or trapped: a host still holding one of
    // this view's source completions must see it cancelled without waiting
    // for a turn of its own. Workers retain their independent lifetimes.
    let _cancel = cancel.clone().drop_guard();
    let ViewSources {
        // Spent on the embedder's thread, where `create_lynx_view` resolved
        // `entry` and `background_entry` against it. Its parsed form rides in
        // `outbox`, for the realms' synchronous loads.
        base_url: _,
        config,
        // Spent on the embedder's thread: the fonts and the default family
        // became `text_context` above, and these two became the requests
        // whose answers `startup` carries, beside the URL each was named by.
        fonts: _,
        default_font_family: _,
        style_sheets: _,
        entry: _,
        background_entry,
        init_data,
        initial_processor,
        global_props,
        screen,
    } = sources;
    let ingredients = DocumentIngredients {
        viewport,
        config,
        text_context,
        style_pool: context.style_pool.clone(),
    };
    let StartupSources { sheets, entry } = startup;
    // What boot imports the entry by, and so what `load_entry` completes.
    let entry_url = entry.url.clone();
    let page = Page::new(context, outbox, metrics.clone(), cancel);
    // Queued before the first task of this view is spawned, so it is the first
    // job of the view and nothing it owns can be served ahead of it. Nothing
    // is waited for here: the realm opens now and boot creates the document
    // at once, which is what lets the whole of boot run while the embedder
    // builds this view's painter. The answer is dropped because there is
    // nothing to do with it — a job is queued by the call rather than by the
    // future it hands back, and `run_job` is what reports a trap inside it.
    drop(run_job(&page, move |page| {
        page.open_realm(
            ingredients,
            RealmStartup {
                sheets,
                screen,
                entry: entry_url,
                background_entry,
                initial_processor,
                init_data,
                global_props,
                native_modules,
            },
        );
        Some(())
    }));
    // The entry is a task, entering the realm as its answer arrives and
    // therefore behind `open_realm`. The sheets went to the realm with the
    // rest of its startup: boot's first flush waits for each, in listed
    // order, and they need no task of their own because the fetcher answers
    // them without one.
    owner::spawn(&page, load_entry(Rc::clone(&page), entry));
    owner::spawn(&page, consume_commands(Rc::clone(&page), commands));
    owner::spawn(&page, consume_metrics(Rc::clone(&page), metrics));
    owner::run_owner(&page).await;
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
            owner::end(&page);
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
    owner::end(&page);
}

/// The one consumer of this view's metrics watch: one settle per change.
///
/// A settle rather than an operation, because there is nothing to do in the
/// realm — the epilogue's `commit_if_dirty` reads the watch itself, adopts
/// whatever it holds and commits if that moved the viewport. What this task
/// exists for is the case where nothing else would enter the realm at all: a
/// painter that resizes while the page is idle.
///
/// A receiver of its own, because `borrow_and_update` marks a value seen for
/// one receiver alone: the document's clone and this one are independent
/// readers of the same state.
///
/// While the page is still loading [`owner::enter`] answers `None` at once and
/// nothing happens, which is right — a document that does not exist yet reads
/// the watch when it is created.
async fn consume_metrics(page: Rc<Page>, mut metrics: watch::Receiver<Option<Viewport>>) {
    loop {
        tokio::select! {
            biased;
            () = page.lifetime.token().cancelled() => return,
            changed = metrics.changed() => if changed.is_err() {
                // The view's seat is gone, so no further metrics can arrive.
                return;
            },
        }
        <Page as Settles>::settle(&page).await;
    }
}

/// The view's MTS entry: the answer to the request `create_lynx_view` made,
/// completed into the realm as the module boot imports by the entry's URL.
///
/// A task like [`owner::load_module`], and for the same reason: the answer may take
/// as long as the fetcher likes, and waiting for it on a task parks nothing —
/// no job of this view's, no job of a sibling's. Its entry into the realm is
/// queued behind `open_realm`, the view's first job, so the realm it completes
/// the module in always exists. Boot's `import` of the entry may have been
/// made already, in which case this is what resumes it, or not yet, in which
/// case the import finds the module in the registry.
///
/// The entry is read here, before any of it runs, by
/// [`owner::module_answer`] as every import's answer is, so an entry that
/// cannot be loaded fails the boot: a load the fetcher could not make is
/// reported as the fetcher's own error, and an answer that is not a script,
/// or a script whose response URL is not an absolute URL, as a `Script` error
/// naming the URL, each as `StartupFailed`. The module is not completed
/// then — the view has ended, and boot's `import` of it is released with the
/// realm. The entry's *evaluation* is the app's code: boot catches what it
/// throws, so a failure there, or in a module it imports, is reported as
/// `ScriptRunError` and boot goes on past it. The rest of boot runs in this
/// same entry, and a failure of the engine's own boot code there — most often
/// a listed sheet boot's flush could not load — is reported twice, with one
/// message: this entry reports the rejection its checkpoint returned as
/// `ScriptRunError`, and the epilogue then reports boot's failure as
/// `StartupFailed`. Naming the entry, which is engine code too, fails the
/// boot from here directly.
///
/// Except where the embedder released the view meanwhile. Completing the entry
/// evaluates it and runs the rest of boot, and an entry that adopts a
/// stylesheet, like boot's own flush waiting for the listed sheets or the
/// binding, parks inside that evaluation on a wait whose first arm is the
/// view's token, so a release makes the evaluation throw; a release also
/// drops the answer's completion, which fails the load. Either is the end of a
/// view nobody is watching rather than a failure of it, and is not reported.
async fn load_entry(page: Rc<Page>, entry: StartupSource) {
    let StartupSource { url, answer } = entry;
    let answered = owner::module_answer(&url, owner::await_source(answer).await).and_then(
        |(response, source)| absolute_response(&url, response).map(|response| (response, source)),
    );
    let completing = Rc::clone(&page);
    owner::enter(&page, move |runtime, js| {
        let completed = answered
            .and_then(|(response_url, source)| runtime.complete_entry(js, &response_url, &source));
        match completed {
            Ok(Ok(())) => {}
            _ if completing.outbox.is_cancelled() => owner::end(&completing),
            Ok(Err(error)) => policy::report(&completing, Scene::Boot, error.into_script_error()),
            // The Open row, over an error that need not be a script's: a load
            // the fetcher could not make is reported as the fetcher's own
            // error, which `StartupFailed` carries as it is.
            Err(error) => owner::terminal(&completing, EngineEvent::StartupFailed(error)),
        }
    })
    .await;
}

/// The entry's response URL, which must be an absolute URL, or the startup
/// failure that is: a `Script` error naming `url`, the URL the entry was
/// requested by, and the response URL.
///
/// It becomes `__Card__`, the base every `new Worker` URL is joined to by URL
/// rules, and a join to a base that does not parse fails for every
/// specifier, boot's own `bobcat:bts` included.
fn absolute_response(url: &str, response: String) -> Result<String, LynxViewError> {
    match url::Url::parse(&response) {
        Ok(_) => Ok(response),
        Err(_) => Err(LynxViewError::Script(platform_script_error(format!(
            "the fetcher answered {url} from {response:?}, which is not an absolute URL"
        )))),
    }
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
    owner::enter(&page, move |runtime, _| {
        runtime.register_font_face(&family, blob);
    })
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
        // Reported inside the entry, before its epilogue, as every other kind
        // of entry reports its failure. Boot can settle in here, when the MTS
        // entry's top-level await was waiting on this worker, and the error
        // is then the first rejection of the ones boot left: the entry's own
        // throw where it threw, or boot's own failure, which the epilogue goes
        // on to report as StartupFailed too.
        let delivering = Rc::clone(&page);
        owner::enter(&page, move |runtime, js| {
            if let Err(error) = runtime.dispatch_worker_event(js, key, payload) {
                policy::report(&delivering, Scene::Listener, error.into_script_error());
            }
        })
        .await;
    }
}

/// Asks the host for one source and waits for it. A fetcher that dropped the
/// request without answering is a failed load, not a wait forever.
async fn request_source(
    outbox: &ViewOutbox,
    request: SourceRequest,
) -> Result<LoadedSource, LynxViewError> {
    owner::await_source(outbox.request_source(request)).await
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod page_tests;

#[cfg(test)]
#[path = "font_face_tests.rs"]
mod font_face_tests;
