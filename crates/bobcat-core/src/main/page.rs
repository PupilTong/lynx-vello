//! One view on `bobcat-main`: the page every task of that view acts on, and
//! the one boundary each of them enters JavaScript through.
//!
//! # One task per wait
//!
//! A view is not one task with a wait for everything. It is a set of tasks on
//! the group's `LocalSet`, one per thing that can be waited for, and tokio is
//! what polls, parks and wakes them:
//!
//! - the owner, [`serve_view`], which builds the page, spawns the rest, and then has exactly one
//!   wait of its own — the view's [`Lifetime`], which is the end or the next task of the view to
//!   finish;
//! - [`consume_commands`], the one ordered consumer of the command stream;
//! - [`boot_page`], the page's boot future: the sheets in cascade order, the entry, and then the
//!   realm;
//! - one [`load_module`] future per resource load an import produced;
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
//! runs one synchronous operation and then settles what that operation left
//! owing: due timers, the commit, the boot report, the `BeginFrame`
//! acknowledgement, the module requests the entry produced, the next timer
//! deadline. [`Page::settle`] is the epilogue alone, for a wake that carries
//! no operation of its own. [`Page::open_realm`] is the one documented
//! exception, because the realm it would enter does not exist until it
//! returns; it runs under the same guard and reports the same way.
//!
//! # Ordering
//!
//! The `LocalSet` is one thread and every entry is synchronous inside
//! [`Page::enter`], so entries never interleave: each stream is consumed in
//! order by its one consumer, and a burst of commands is one entry, one
//! commit and one acknowledgement. Module completions, timer wakes and a
//! sibling's checkpoint are independent tasks and may run between any two
//! bursts.
//!
//! # The end
//!
//! One [`Lifetime`] per view carries the tasks, the token that ends them and
//! the latch this thread reads. The token is the embedder's: `LynxView::drop`
//! and a fatal `pump` event cancel it, [`serve_view`]'s drop guard cancels it
//! on every exit, and each worker this view's realm creates holds a child of
//! it. What ended a view is not recorded anywhere, because nothing reads it:
//! what the embedder was told is whatever was reported before the end, and a
//! release is the token having been cancelled from outside.
//!
//! Between an embedder-side cancel and the owner's turn a task may still run
//! one entry, because only this thread writes the latch. A command queued
//! behind a release lands in exactly that window — its wake is served before
//! the owner's — so the discard cannot rest on the owner running first:
//! [`consume_commands`] reads the token itself, once per burst, at the wake
//! boundary. A burst already inside [`Page::apply`] when the cancel lands
//! finishes, the way synchronous JavaScript already executing does.
//!
//! # Borrows
//!
//! No `RefCell` borrow and no borrow of the shared script runtime is ever
//! held across an `.await`. Every borrow is taken inside a synchronous method
//! and released before it returns, which is why [`Page`] is reachable from
//! every task of its view at once.
//!
//! # Waits
//!
//! After this module every `select!` in this crate is one of four kinds, and
//! each is a wait rather than a dispatcher:
//!
//! - **thread lifetime** — `group_task` and `serve_workers`, each waiting on attach versus join;
//! - **an object's lifetime** — one [`Lifetime::serve`] per view and per worker, waiting on the end
//!   versus the next task of that object to finish;
//! - **a realm's clock** — one [`serve_clock`] per live realm, a view's and a worker's alike,
//!   waiting on its deadline, the re-arm that moves it, and a sibling's checkpoint;
//! - **the worker's pre-boot wait** — its script versus a `Terminate` that must win, or its parent
//!   view being released.
//!
//! How many there are is the group's shape rather than a constant: one of the
//! first kind per engine thread, one of the second per live view and per live
//! worker, one of the third per live realm, one of the fourth per worker that
//! has not booted yet. `link.rs`'s `block_on_deadline` is a hand-rolled poll
//! loop rather than a select.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::quickjs::ScriptRuntime;
use super::runtime::{DocumentIngredients, MainThreadRuntime, PageData};
use super::{AttachedView, GroupContext};
use crate::background::WorkerEvent;
#[cfg(test)]
use crate::clock::ClockInstant;
use crate::lifetime::{EndOnUnwind, Lifetime, Settles, serve_clock};
use crate::link::{SourceAnswer, ToMain, ViewOutbox};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::threads::panicked;
use crate::view::{EngineEvent, LynxViewError, ViewSources, Viewport};

/// What the page is, which is the only thing that decides what a command can
/// do to it.
///
/// Both live states are boxed: this enum is one field of a page that every
/// task of a view holds, and a page is in each of them for exactly one phase
/// of its life, so carrying the larger of the two inline would cost every
/// page the whole of the other.
enum Realm {
    /// No realm and no document yet: the boot module has not run, so the two
    /// commands that describe a document write into the ingredients it will
    /// be built from and the rest have nowhere to go.
    Loading(Box<DocumentIngredients>),
    Live(Box<MainThreadRuntime>),
    /// The realm could not be opened, or the view is over and the owner has
    /// reclaimed it. A terminal state, so every entry point is total.
    Gone,
}

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
    realm: RefCell<Realm>,
    /// The newest `BeginFrame` sequence applied and not yet acknowledged.
    ///
    /// Acknowledged in the epilogue rather than where it is applied: a host
    /// blocked on this number is waiting for the frame it implies, which the
    /// epilogue's commit is what publishes.
    pending_begin_frame: Cell<Option<u64>>,
    boot_reported: Cell<bool>,
    /// Every task of this view, the token that ends them, the latch this thread
    /// reads, and the two numbers this realm's clock task waits on — the
    /// deadline it armed and the generation its own last entry recorded. A
    /// worker this view's realm creates carries a child of that token, so
    /// releasing the view ends the workers it made.
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
/// document's ingredients staged.
struct BootSources {
    style_sheets: Vec<String>,
    entry: String,
    background_entry: Option<String>,
    /// The host's page data, as JSON text only the realm reads.
    init_data: Option<String>,
    global_props: Option<String>,
}

impl Page {
    /// A loading page, over the token that ends it.
    fn new(
        context: Rc<GroupContext>,
        outbox: ViewOutbox,
        ingredients: DocumentIngredients,
        token: CancellationToken,
    ) -> Rc<Self> {
        Rc::new(Self {
            context,
            outbox,
            realm: RefCell::new(Realm::Loading(Box::new(ingredients))),
            pending_begin_frame: Cell::new(None),
            boot_reported: Cell::new(false),
            lifetime: Lifetime::new(token),
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

    fn is_live(&self) -> bool {
        matches!(&*self.realm.borrow(), Realm::Live(_))
    }

    /// Runs one synchronous operation against the live realm and settles what
    /// it owes.
    ///
    /// This is the one way into JavaScript. `None` is a realm that is not
    /// live — still loading, or gone — or a view that has ended; either way
    /// the operation never ran.
    ///
    /// The operation *and* the whole epilogue run under one `catch_unwind`,
    /// because the bridge erases a panic into "the host function panicked"
    /// and a panic on this thread is the view's failure rather than the
    /// group's.
    fn enter<T>(
        self: &Rc<Self>,
        operation: impl FnOnce(&mut MainThreadRuntime, &mut ScriptRuntime) -> T,
    ) -> Option<T> {
        if self.ended() {
            return None;
        }
        let entered = catch_unwind(AssertUnwindSafe(|| {
            let js = &mut *self.context.js.borrow_mut();
            let mut realm = self.realm.borrow_mut();
            let Realm::Live(runtime) = &mut *realm else {
                return None;
            };
            let value = operation(runtime, js);
            self.epilogue(runtime, js);
            Some(value)
        }));
        match entered {
            Ok(value) => value,
            Err(payload) => {
                self.trapped(payload.as_ref());
                None
            }
        }
    }

    /// The epilogue alone, for a wake that carries no operation of its own —
    /// a timer deadline, or a sibling's checkpoint.
    fn settle(self: &Rc<Self>) {
        self.enter(|_, _| ());
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
    /// 3. **The boot report**, once, so the frame exists before the event that implies it.
    /// 4. **The `BeginFrame` acknowledgement**, for the same reason: a host blocked on the sequence
    ///    number is blocked on that frame.
    /// 5. **The module requests** this entry produced, each spawned as a load of its own.
    /// 6. **The next timer deadline**, republished only when it moved.
    /// 7. **The checkpoint generation**, so the clock task can tell this page's own bumps from a
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
        if !self.boot_reported.get() {
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
        self.lifetime.arm_deadline(runtime.next_timer_deadline());
        self.lifetime.record_checkpoint(js.checkpoint_generation());
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
    /// burst inside one [`Self::enter`]; a loading one writes what it can
    /// into the ingredients its document will be built from; a page that has
    /// ended drops the burst, `BeginFrame` included, because the end has
    /// already acknowledged the pending one.
    ///
    /// The burst loop checks the latch before each command. It cannot be
    /// truncated from another thread — only this thread writes that latch — so
    /// one entry is still one commit; what it stops is the rest of a burst
    /// behind a command that ended the view.
    fn apply(self: &Rc<Self>, commands: impl Iterator<Item = ToMain>) {
        if self.ended() {
            return;
        }
        // Taken before the entry below, and eagerly, because an iterator
        // adapter would run it inside that entry: what the seam spawns has to
        // trap the way any other task of this view does, rather than into the
        // `catch_unwind` a live entry runs under.
        #[cfg(test)]
        let commands = self.take_test_seams(commands);
        if self.is_live() {
            self.enter(|runtime, js| {
                for command in commands {
                    if self.ended() {
                        break;
                    }
                    self.apply_command(runtime, js, command);
                }
            });
            return;
        }
        self.stage(commands);
    }

    /// Applies one command to a booted view.
    fn apply_command(
        &self,
        runtime: &mut MainThreadRuntime,
        js: &mut ScriptRuntime,
        command: ToMain,
    ) {
        match command {
            ToMain::PageUpdate(update) => {
                if let Err(error) = runtime.apply_page_update(js, update) {
                    self.fail(EngineEvent::ScriptRunError(error.into_script_error()));
                }
            }
            ToMain::DispatchEvent {
                target,
                name,
                detail,
            } => {
                // A listener that panics is not fatal to the view: the
                // payload is dropped, the rest of the burst applies, and the
                // epilogue still runs.
                let delivered = catch_unwind(AssertUnwindSafe(|| {
                    runtime.dispatch_event(js, target, name, &detail)
                }));
                if let Ok(Err(error)) = delivered {
                    self.outbox
                        .engine_event(EngineEvent::ListenerFailed(error.into_script_error()));
                }
            }
            ToMain::Resize {
                width,
                height,
                device_pixel_ratio,
            } => runtime.apply_resize(width, height, device_pixel_ratio),
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
    /// What a command can do here is narrow: the two that describe the
    /// document write into the ingredients it will be built from, a
    /// `BeginFrame` is acknowledged at once so an offscreen host is never
    /// blocked by a load, and nothing else has anywhere to go. Dropping a
    /// `Probe` drops the sender it captured, which answers the probing test
    /// `None` rather than leaving it to wait out its deadline.
    fn stage(&self, commands: impl Iterator<Item = ToMain>) {
        let mut acknowledged: Option<u64> = None;
        {
            let mut realm = self.realm.borrow_mut();
            let Realm::Loading(ingredients) = &mut *realm else {
                return;
            };
            for command in commands {
                match command {
                    // The public global-event API only enqueues on a ready view.
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
                    ToMain::DispatchEvent { .. } | ToMain::Refill { .. } => {}
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

    /// Stages one author sheet, in cascade order. `false` is a page that is
    /// no longer loading, which has already ended.
    fn stage_sheet(&self, sheet: crate::resource::StyleSheetSource) -> bool {
        let mut realm = self.realm.borrow_mut();
        let Realm::Loading(ingredients) = &mut *realm else {
            return false;
        };
        ingredients.sheets.push(sheet);
        true
    }

    /// Opens this view's realm and runs its entry, then starts the waits that
    /// only a live realm has.
    ///
    /// The one entry into JavaScript that is not [`Self::enter`], because the
    /// realm it would enter does not exist until this returns. Everything up
    /// to and including the entry is one synchronous stretch under both
    /// borrows, so the shared runtime is borrowed for exactly this and
    /// released again; the checkpoint receiver is created while that borrow
    /// is still held, so no sibling's bump between boot and the clock task's
    /// first poll can be lost.
    fn open_realm(
        self: &Rc<Self>,
        source: &str,
        url: &str,
        init_data: Option<String>,
        global_props: Option<String>,
        background_entry: Option<String>,
    ) {
        // A view that has already ended builds no realm and runs no entry:
        // its tasks are about to be reclaimed, and the ingredients go with the
        // page rather than into a document nobody will ever see.
        if self.ended() {
            *self.realm.borrow_mut() = Realm::Gone;
            return;
        }
        let opened = catch_unwind(AssertUnwindSafe(|| {
            let js = &mut *self.context.js.borrow_mut();
            let mut realm = self.realm.borrow_mut();
            // Out of `Loading` before any failure path can report: the
            // ingredients are spent either way, and a page whose realm could
            // not be opened is over.
            let Realm::Loading(ingredients) = std::mem::replace(&mut *realm, Realm::Gone) else {
                return None;
            };
            let (mut runtime, worker_events) = match MainThreadRuntime::new(
                js,
                *ingredients,
                self.outbox.clone(),
                &self.context.workers,
                url,
                background_entry,
                PageData {
                    init_data,
                    global_props,
                },
            ) {
                Ok(opened) => opened,
                Err(error) => return Some(Err(error.into_script_error().into())),
            };
            // The flag is written from the embedder's own thread, so it can
            // change between two statements of this stretch.
            if self.outbox.is_cancelled() {
                return None;
            }
            if let Err(error) = runtime.run_main_thread_script(js, source, url) {
                if self.outbox.is_cancelled() {
                    return None;
                }
                return Some(Err(error.into_script_error().into()));
            }
            let checkpoints = js.checkpoints();
            self.lifetime.record_checkpoint(js.checkpoint_generation());
            *realm = Realm::Live(Box::new(runtime));
            Some(Ok((worker_events, checkpoints)))
        }));
        let opened = match opened {
            Ok(opened) => opened,
            Err(payload) => return self.trapped(payload.as_ref()),
        };
        match opened {
            // Nobody is listening for this view any more, so there is nobody
            // to report to.
            None => {
                self.end();
            }
            Some(Err(error)) => self.fail(EngineEvent::StartupFailed(error)),
            Some(Ok((worker_events, checkpoints))) => {
                self.spawn(consume_worker_events(Rc::clone(self), worker_events));
                self.spawn(serve_clock(
                    Rc::clone(self),
                    self.lifetime.deadlines(),
                    checkpoints,
                ));
                // A boot that finished synchronously reports here, and the
                // module requests its entry left are spawned here.
                self.settle();
            }
        }
    }

    /// This view's whole tail: wait, end, reclaim, release the realm.
    ///
    /// The wait is the lifetime's — the end, or the next task of the view to
    /// finish. [`Self::end`] after it is what mirrors a cancellation that came
    /// from another thread onto this thread's latch, and what acknowledges a
    /// pending `BeginFrame` so a blocked painter is released. The reap is what
    /// makes this the last owner of the page: a task holds an `Rc` of it until
    /// its future is dropped.
    async fn run_owner(self: &Rc<Self>) {
        self.lifetime
            .serve(&mut |payload| self.trapped(payload.as_ref()))
            .await;
        self.end();
        self.lifetime
            .reap(&mut |payload| self.trapped(payload.as_ref()))
            .await;
        // JavaScript first, then the Rust object it named: `MainThreadRuntime`
        // drops its fields in declaration order, and this is where that
        // happens — after every task that could still have entered the realm
        // is gone.
        *self.realm.borrow_mut() = Realm::Gone;
    }

    /// Takes the one command that is not the realm's out of a burst.
    ///
    /// It spawns two tasks of this view — one that panics on its first poll,
    /// and behind it one that records what [`Self::ended`] says when it is
    /// next polled — which is the only way a test can watch a panic reach a
    /// sibling before it reaches the owner.
    #[cfg(test)]
    fn take_test_seams(
        self: &Rc<Self>,
        commands: impl Iterator<Item = ToMain>,
    ) -> std::vec::IntoIter<ToMain> {
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
        rest.into_iter()
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

/// What this view's clock task and its unwind guard reach it through.
impl Settles for Page {
    fn lifetime(&self) -> &Lifetime {
        &self.lifetime
    }

    fn settle(owner: &Rc<Self>) {
        owner.settle();
    }

    fn end(owner: &Rc<Self>) {
        owner.end();
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
        commands,
        cancel,
    } = view;
    // On every exit path, ordinary or trapped: a host still holding one of
    // this view's source completions must see it cancelled without waiting
    // for a turn of its own, and every worker this view created — each
    // holding a child of this token — must wake and end.
    let _cancel = cancel.clone().drop_guard();
    let ViewSources {
        config,
        fonts,
        default_font_family,
        style_sheets,
        entry,
        background_entry,
        init_data,
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
            global_props,
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
        page.apply(std::iter::once(first).chain(rest));
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
        global_props,
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
    page.open_realm(&source, &url, init_data, global_props, background_entry);
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
    page.enter(|runtime, js| {
        if let Err(error) = runtime.complete_module(js, &url, source) {
            let error = error.into_script_error();
            page.fail(if page.boot_reported.get() {
                EngineEvent::ScriptRunError(error)
            } else {
                EngineEvent::StartupFailed(error.into())
            });
        }
    });
}

/// The one ordered consumer of everything this view's workers say.
///
/// The channel's senders are the realm's `WorkerOwner` and this view's worker
/// tasks, so it closes only once the realm is gone — which is to say, with
/// the view.
async fn consume_worker_events(page: Rc<Page>, mut events: mpsc::UnboundedReceiver<WorkerEvent>) {
    while let Some(WorkerEvent { key, payload }) = events.recv().await {
        let delivered = page.enter(|runtime, js| runtime.dispatch_worker_event(js, key, payload));
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
