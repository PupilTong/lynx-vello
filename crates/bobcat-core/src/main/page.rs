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
//!   wait of its own — the view's end;
//! - [`consume_commands`], the one ordered consumer of the command stream;
//! - [`boot_page`], the page's boot future: the sheets in cascade order, the entry, and then the
//!   realm;
//! - one [`load_module`] future per resource load an import produced;
//! - [`consume_worker_events`], the one ordered consumer of this view's workers;
//! - [`wait_timers`], which owns this realm's one pinned sleep;
//! - [`follow_checkpoints`], which watches the runtime-wide checkpoint generation for a sibling's
//!   entry into JavaScript.
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
//! # Borrows
//!
//! No `RefCell` borrow and no borrow of the shared script runtime is ever
//! held across an `.await`. Every borrow is taken inside a synchronous method
//! and released before it returns, which is why [`Page`] is reachable from
//! every task of its view at once.
//!
//! # Waits
//!
//! After this module every `select!` in this crate is one of three kinds, and
//! each is a wait rather than a dispatcher:
//!
//! - **task lifetime** — `group_task` and `serve_workers`, each waiting on attach versus join;
//! - **a realm's clock** — one [`wait_timers`] per live realm, waiting on its deadline versus the
//!   re-arm that moves it: a view's is here, a worker's is in `background/thread.rs`;
//! - **the worker's pre-boot wait** — its script versus a `Terminate` that must win.
//!
//! How many there are is the group's shape rather than a constant: one of the
//! first kind per engine thread, one of the second per live realm, one of the
//! third per worker that has not booted yet. `link.rs`'s `block_on_deadline`
//! is a hand-rolled poll loop rather than a select.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::rc::Rc;

use tokio::sync::{mpsc, watch};
use tokio::task;

use super::quickjs::ScriptRuntime;
use super::runtime::{DocumentIngredients, MainThreadRuntime};
use super::{AttachedView, GroupContext};
use crate::background::WorkerEvent;
use crate::clock::ClockInstant;
use crate::link::{CancelOnExit, SourceAnswer, ToMain, ViewOutbox};
use crate::resource::{LoadedSource, SourceRequest, unanswered_source};
use crate::threads::{panic_message, platform_script_error};
use crate::view::{EngineEvent, LynxViewError, MainSources, Viewport};

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

/// Why a view ended. The first one wins; all three take the same path from
/// there, and what differs is only what the embedder has already been told.
enum End {
    /// The embedder dropped the view — its command channel closed — or
    /// cancelled it before the realm opened.
    Released,
    /// A failure this view has already been reported.
    Failed,
    /// A task of this view panicked, and nothing has been reported yet.
    ///
    /// The payload is not reachable from the guard that ends the view — a
    /// `Drop` is handed no payload — so the report is the owner's, out of the
    /// handle it awaits. That handle can be gone by then, because [`Page::spawn`]
    /// prunes the handles of tasks that have finished, and this reason is what
    /// is left of the panic when it is.
    Trapped,
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
    /// The runtime-wide checkpoint generation as of this page's own last
    /// entry, which is what lets [`follow_checkpoints`] ignore its own bumps.
    own_checkpoint: Cell<u64>,
    /// This realm's earliest armed timer, for [`wait_timers`]. Republished
    /// only when it moves, so no `Sleep` is rebuilt for a wake that changed
    /// nothing.
    deadline: watch::Sender<Option<ClockInstant>>,
    /// Every task of this view, so the owner can reclaim all of them.
    tasks: RefCell<Vec<task::JoinHandle<()>>>,
    /// The first end wins; the owner awaits the receiving end.
    end: mpsc::UnboundedSender<End>,
    /// What makes an end immediate. Everything checks it first, so the abort
    /// that follows only reclaims the tasks rather than being what stops them.
    ended: Cell<bool>,
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
    init_data: Option<serde_json::Value>,
    global_props: Option<serde_json::Value>,
}

impl Page {
    /// A loading page and the owner's end of the channel that says it is
    /// over.
    fn new(
        context: Rc<GroupContext>,
        outbox: ViewOutbox,
        ingredients: DocumentIngredients,
    ) -> (Rc<Self>, mpsc::UnboundedReceiver<End>) {
        let (end, ended) = mpsc::unbounded_channel();
        let page = Self {
            context,
            outbox,
            realm: RefCell::new(Realm::Loading(Box::new(ingredients))),
            pending_begin_frame: Cell::new(None),
            boot_reported: Cell::new(false),
            own_checkpoint: Cell::new(0),
            deadline: watch::channel(None).0,
            tasks: RefCell::new(Vec::new()),
            end,
            ended: Cell::new(false),
            #[cfg(test)]
            epilogues: Cell::new(0),
        };
        (Rc::new(page), ended)
    }

    /// Starts one more task of this view, keeping the handle so the owner can
    /// reclaim it.
    ///
    /// A panic anywhere in the task ends the view: the guard's `Drop` runs
    /// during the unwind, before tokio's task harness catches it, so the
    /// owner is already awake by the time the handle reports the panic.
    fn spawn(self: &Rc<Self>, future: impl Future<Output = ()> + 'static) {
        let page = Rc::clone(self);
        let handle = task::spawn_local(async move {
            let _guard = EndOnUnwind(page);
            future.await;
        });
        let mut tasks = self.tasks.borrow_mut();
        // A task that has already returned holds nothing of this view any
        // more, and a task that trapped has already ended the view above.
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    /// Ends this view, once. `true` for the call that did it.
    ///
    /// Synchronous, because the flag is what makes an end immediate:
    /// [`Self::enter`], [`Self::apply`] and every timer wake return at once
    /// when it is set, and the owner's abort only reclaims the tasks.
    fn end(&self, reason: End) -> bool {
        if self.ended.replace(true) {
            return false;
        }
        // A painter blocked in `wait_begin_frame` is released rather than
        // timed out: the frame it was waiting for will never come.
        if let Some(seq) = self.pending_begin_frame.take() {
            self.outbox.begin_frame_serviced(seq);
        }
        self.deadline
            .send_if_modified(|deadline| deadline.take().is_some());
        let _ = self.end.send(reason);
        true
    }

    fn ended(&self) -> bool {
        self.ended.get()
    }

    /// Reports one fatal failure and ends the view, in that order and once.
    ///
    /// It touches no realm borrow, so an operation may call it from inside
    /// [`Self::enter`]: the epilogue that follows sees the end and does
    /// nothing, which is what keeps one failure one report.
    fn fail(&self, event: EngineEvent) {
        if self.end(End::Failed) {
            self.outbox.engine_event(event);
        }
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
    /// 7. **The checkpoint generation**, so the follower can tell this page's own bumps from a
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
        let deadline = runtime.next_timer_deadline();
        self.deadline.send_if_modified(|armed| {
            if *armed == deadline {
                return false;
            }
            *armed = deadline;
            true
        });
        self.own_checkpoint.set(js.checkpoint_generation());
    }

    /// Reports a panic this thread caught as the failing view's, the same
    /// wording the owner uses for a task that trapped.
    fn trapped(&self, payload: &(dyn std::any::Any + Send)) {
        let message = format!("the Lynx main thread panicked: {}", panic_message(payload));
        self.fail(EngineEvent::ScriptRunError(platform_script_error(message)));
    }

    /// Applies one burst of commands: one entry, one commit, one
    /// acknowledgement.
    ///
    /// Total over every state a page can be in. A live page takes the whole
    /// burst inside one [`Self::enter`]; a loading one writes what it can
    /// into the ingredients its document will be built from; a page that has
    /// ended drops the burst, `BeginFrame` included, because the end has
    /// already acknowledged the pending one.
    fn apply(self: &Rc<Self>, commands: impl Iterator<Item = ToMain>) {
        if self.ended() {
            return;
        }
        if self.is_live() {
            self.enter(|runtime, js| {
                for command in commands {
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
    /// is still held, so no sibling's bump between boot and the follower's
    /// first poll can be lost.
    fn open_realm(
        self: &Rc<Self>,
        source: &str,
        url: &str,
        init_data: Option<&serde_json::Value>,
        global_props: Option<&serde_json::Value>,
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
            let mut runtime = match MainThreadRuntime::new(js, *ingredients, self.outbox.clone()) {
                Ok(runtime) => runtime,
                Err(error) => return Some(Err(error.into_script_error().into())),
            };
            // The flag is written from the embedder's own thread, so it can
            // change between two statements of this stretch.
            if self.outbox.is_cancelled() {
                return None;
            }
            if let Err(error) = runtime.prepare_initial_data(init_data, global_props) {
                return Some(Err(error.into_script_error().into()));
            }
            if self.outbox.is_cancelled() {
                return None;
            }
            let worker_events = match runtime.install_workers(
                js,
                &self.context.workers,
                self.outbox.clone(),
                url,
                background_entry,
            ) {
                Ok(events) => events,
                Err(error) => return Some(Err(error.into_script_error().into())),
            };
            if let Err(error) = runtime.run_main_thread_script(js, source, url) {
                if self.outbox.is_cancelled() {
                    return None;
                }
                return Some(Err(error.into_script_error().into()));
            }
            let checkpoints = js.checkpoints();
            self.own_checkpoint.set(js.checkpoint_generation());
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
                self.end(End::Released);
            }
            Some(Err(error)) => self.fail(EngineEvent::StartupFailed(error)),
            Some(Ok((worker_events, checkpoints))) => {
                self.spawn(consume_worker_events(Rc::clone(self), worker_events));
                self.spawn(wait_timers(Rc::clone(self), self.deadline.subscribe()));
                self.spawn(follow_checkpoints(Rc::clone(self), checkpoints));
                // A boot that finished synchronously reports here, and the
                // module requests its entry left are spawned here.
                self.settle();
            }
        }
    }

    /// Reclaims every task of this view, reports a panic once, then releases
    /// the realm.
    ///
    /// The abort is what makes a parked task return; the end flag is what
    /// already stopped it from doing anything. Awaiting each handle is what
    /// makes this the last owner of the page: a task holds an `Rc` of it
    /// until its future is dropped.
    ///
    /// The report does not depend on a handle still being here. A handle that
    /// panicked carries the payload, which is the better message; a handle
    /// [`Self::spawn`] pruned once it had finished carries nothing, and
    /// [`End::Trapped`] is what is left of that panic. Either way the embedder
    /// hears about it exactly once.
    async fn reap(&self, reason: End) {
        let tasks = std::mem::take(&mut *self.tasks.borrow_mut());
        for task in &tasks {
            task.abort();
        }
        let mut trapped: Option<String> = None;
        for task in tasks {
            let Err(error) = task.await else { continue };
            if error.is_panic() && trapped.is_none() {
                trapped = Some(format!(
                    "the Lynx main thread panicked: {}",
                    panic_message(error.into_panic().as_ref())
                ));
            }
        }
        let message = trapped.or_else(|| {
            matches!(reason, End::Trapped).then(|| "the Lynx main thread panicked".to_owned())
        });
        if let Some(message) = message {
            self.outbox
                .engine_event(EngineEvent::ScriptRunError(platform_script_error(message)));
        }
        // JavaScript first, then the Rust object it named: `MainThreadRuntime`
        // drops its fields in declaration order, and this is where that
        // happens — after every task that could still have entered the realm
        // is gone.
        *self.realm.borrow_mut() = Realm::Gone;
    }

    /// How many tasks this view still has, for a test that pins an end
    /// reaching every one of them.
    #[cfg(test)]
    pub(super) fn task_count(&self) -> usize {
        self.tasks.borrow().len()
    }

    /// How many times the epilogue has run.
    #[cfg(test)]
    pub(super) fn epilogue_count(&self) -> u64 {
        self.epilogues.get()
    }
}

/// Ends the view if the task it guards is unwinding.
///
/// Its `Drop` runs during the unwind, before tokio's task harness catches it,
/// so the owner is awake before the handle it will await reports the panic.
struct EndOnUnwind(Rc<Page>);

impl Drop for EndOnUnwind {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // The payload is not reachable from here, so the reason is all
            // this can say; the owner turns it into the report.
            self.0.end(End::Trapped);
        }
    }
}

/// One view's whole life on this thread: build the page, spawn its waits,
/// wait for the end, reclaim.
///
/// Task lifetime and nothing else. No command, no source, no timer and no
/// frame is decided here.
pub(super) async fn serve_view(context: Rc<GroupContext>, view: AttachedView, outbox: ViewOutbox) {
    // On every exit path, ordinary or trapped: a host still holding one of
    // this view's source completions must see it cancelled without waiting
    // for a turn of its own.
    let _cancel = CancelOnExit(view.cancel.clone());
    let AttachedView {
        viewport,
        sources,
        commands,
        ..
    } = view;
    let MainSources {
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
    let (page, mut ended) = Page::new(context, outbox, ingredients);
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
    // The owner's one wait. `None` cannot happen while the page holds the
    // sending end, and every reason takes the same path from here: what
    // differs is only what the view was already told, and a `Failed` end was
    // reported before it was sent. `Trapped` is the one the owner still owes
    // a report for, which is why the reason is read rather than dropped.
    let reason = ended.recv().await.unwrap_or(End::Released);
    page.reap(reason).await;
}

/// The one ordered consumer of this view's command stream.
///
/// A burst rather than a command: the rest of what is already queued goes
/// with the first, so a host's whole round of input is one commit and one
/// acknowledgement. Bounded by the snapshot the count was taken from, so a
/// producer faster than this task cannot starve the epilogue.
async fn consume_commands(page: Rc<Page>, mut commands: mpsc::UnboundedReceiver<ToMain>) {
    while let Some(first) = commands.recv().await {
        let queued = commands.len();
        let rest = std::iter::from_fn(|| commands.try_recv().ok()).take(queued);
        page.apply(std::iter::once(first).chain(rest));
    }
    // The embedder released this view. Everything it owns goes with the
    // tasks the owner is about to reclaim.
    page.end(End::Released);
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
            page.end(End::Released);
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
        page.end(End::Released);
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
        page.end(End::Released);
        return;
    }
    page.open_realm(
        &source,
        &url,
        init_data.as_ref(),
        global_props.as_ref(),
        background_entry,
    );
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
        page.enter(|runtime, js| {
            if let Err(error) = runtime.dispatch_worker_event(js, key, payload) {
                page.outbox
                    .engine_event(EngineEvent::ListenerFailed(error.into_script_error()));
            }
        });
    }
}

/// This realm's one wait on its own clock.
///
/// The `select!` below is this task's own wait — a deadline, or the re-arm
/// that moves it — and dispatches nothing: when a deadline passes, the page's
/// epilogue is what fires the timers it named. One `Sleep` for the whole
/// task, re-armed only when the deadline moved, because a `Sleep` registers
/// with the platform's timer on its first poll and one per wake would leave a
/// registration behind per wake.
async fn wait_timers(page: Rc<Page>, mut deadlines: watch::Receiver<Option<ClockInstant>>) {
    let mut armed: Option<ClockInstant> = None;
    let mut sleep = pin!(crate::clock::sleep_until(ClockInstant::now()));
    loop {
        if page.ended() {
            return;
        }
        // Copied out: nothing holds a `Ref` of the watch across the awaits
        // below.
        let deadline = *deadlines.borrow_and_update();
        if deadline != armed {
            if let Some(deadline) = deadline {
                sleep.set(crate::clock::sleep_until(deadline));
            }
            armed = deadline;
        }
        match armed {
            // Nothing is armed, and an ended page publishes `None`, which is
            // what parks this task until the abort lands.
            None => {
                if deadlines.changed().await.is_err() {
                    return;
                }
            }
            Some(_) => {
                tokio::select! {
                    () = &mut sleep => {
                        // Consumed: the next turn arms a wait of its own
                        // rather than polling this one again.
                        armed = None;
                        page.settle();
                    }
                    changed = deadlines.changed() => if changed.is_err() { return },
                }
            }
        }
    }
}

/// Watches the runtime-wide checkpoint generation for a sibling's entry into
/// JavaScript.
///
/// The promise-job queue is the runtime's, so a sibling's checkpoint runs this
/// realm's jobs too: an import of this page's may have finished inside an
/// entry that had nothing to do with it. The epilogue is what spawns the
/// requests those continuations produced and commits what they changed.
/// Equality with this page's own generation is what keeps its own bumps from
/// waking it.
async fn follow_checkpoints(page: Rc<Page>, mut checkpoints: watch::Receiver<u64>) {
    while checkpoints.changed().await.is_ok() {
        if page.ended() {
            return;
        }
        if *checkpoints.borrow_and_update() == page.own_checkpoint.get() {
            continue;
        }
        page.settle();
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
