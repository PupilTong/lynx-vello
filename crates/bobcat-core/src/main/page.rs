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
//! timers, the commit, the boot report, the frame-post acknowledgement, the
//! module requests and future settles the entry produced, the `@font-face`
//! loads, the next timer deadline. Before the operation it moves the
//! document's animation clock to the painter's, so whatever the operation
//! restyles computes from the current instant. [`Settles::settle`] is the
//! epilogue alone, for a wake that carries no operation of its own.
//! [`Page::open_realm`] is a job too, and the one before the realm exists
//! that does not go through `enter`, because the realm it would enter does
//! not exist until it returns. After the end, the disposal exchange the page
//! runs as its `before_release` hook and the release of its realm in
//! [`owner::run_owner`] are [`owner::after_end`] jobs, which run past the
//! latch and the epilogue.
//!
//! The epilogue's steps, and their order, are the driver's
//! ([`crate::realm::owner`]). What the driver is told about a page is the
//! page's [`RealmOwner`] impl: where the realm, the runtime and the host are,
//! that its reports are engine events, the epilogue steps only a page has —
//! the commit and the deliveries it posts, `ScriptFinished`, the frame-post
//! acknowledgement, the `@font-face` loads — and the three things only a page
//! owes — the animation clock moved to the painter's before an entry's
//! operation, the acknowledgement of a pending frame post at the end, and the
//! JavaScript disposal before the release.
//!
//! What a failure is reported as, and whether it ends the view, is the MTS
//! table in [`policy`], read by the scene the failure happened in: every
//! report here goes through [`policy::report`], except the disposal's, which
//! is the table's Disposal row, and the Open failures [`load_entry`] meets —
//! an entry the fetcher could not load, one it answered with something other
//! than a script or from a URL that is not absolute, and naming the entry —
//! which are each already a `LynxViewError` and are sent as `StartupFailed`
//! as they are, through [`owner::terminal`].
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
//! acknowledgement. A [`ToMain::Posted`] marker in a burst adopts the
//! painter's scroll offsets where it stands, and the frame the painter
//! posted is applied once, after the last command of the first burst that
//! has applied every command the seat's
//! [`CommandSender`](crate::link::CommandSender) sent before the post (its
//! [`FramePost::fence`]), so the events a painter pass dispatched and the
//! host's own updates run before the frame an acknowledgement implies. A
//! [`ToMain::ModuleCallback`] is not sent through that sender, and neither
//! the sender nor this page counts it, so a frame post does not wait for one
//! sent before it; nor is a [`ToMain::CanvasResized`]. Module completions, timer wakes and a
//! sibling's checkpoint are independent tasks and may queue an entry between any two bursts.
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
//!   posted, termination included, versus the worker's root module finishing until it has;
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
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use terminus::link::NodeId;
use terminus::link::html::canvas::update_placeholders;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::quickjs::{ScriptRuntime, SharedRuntime};
use super::runtime::{Deliveries, DocumentIngredients, MainThreadRuntime, RealmStartup};
use super::{AttachedView, GroupContext};
use crate::background::WorkerEvent;
use crate::clock::ClockInstant;
use crate::jobs::{JobKind, JsThreadName};
use crate::lifetime::{Lifetime, Settles, run_job, serve_clock};
use crate::link::{FramePost, HostOutbox, ScrollEntry, ScrollMailbox, ToMain, ViewOutbox};
use crate::performance::{MemoryCategory, MemoryItem};
use crate::realm::RealmCore;
use crate::realm::owner::{self, RealmOwner};
use crate::realm::policy::{self, Row, Scene};
use crate::resource::{LoadedSource, SourceRequest};
use crate::script::ScriptError;
use crate::threads::platform_script_error;
use crate::view::{
    EngineEvent, LynxViewError, StartupSource, StartupSources, ViewSources, Viewport,
};

pub(super) struct Page {
    context: Rc<GroupContext>,
    outbox: ViewOutbox,
    host: HostOutbox,
    realm: RefCell<Option<Box<MainThreadRuntime>>>,
    metrics: watch::Receiver<Option<Viewport>>,
    worker_events: RefCell<Option<mpsc::UnboundedReceiver<WorkerEvent>>>,
    scroll: Arc<ScrollMailbox>,
    scrolled: RefCell<FxHashMap<NodeId, ScrollEntry>>,
    frame_due: Cell<Option<FramePost>>,
    applied: Cell<u64>,
    pending_begin_frame: Cell<Option<u64>>,
    boot_reported: Cell<bool>,
    content_visibility_posted: Cell<bool>,
    intersections_posted: Cell<bool>,
    resizes_posted: Cell<bool>,
    component_events_posted: Cell<bool>,
    lifetime: Lifetime,
    #[cfg(feature = "devtools")]
    console: std::cell::OnceCell<Rc<crate::devtools::runtime::ConsoleTap>>,
}

impl Page {
    fn new(
        context: Rc<GroupContext>,
        outbox: ViewOutbox,
        metrics: watch::Receiver<Option<Viewport>>,
        scroll: Arc<ScrollMailbox>,
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
            scroll,
            scrolled: RefCell::default(),
            frame_due: Cell::new(None),
            applied: Cell::new(0),
            pending_begin_frame: Cell::new(None),
            boot_reported: Cell::new(false),
            content_visibility_posted: Cell::new(false),
            intersections_posted: Cell::new(false),
            resizes_posted: Cell::new(false),
            component_events_posted: Cell::new(false),
            lifetime,
            #[cfg(feature = "devtools")]
            console: std::cell::OnceCell::new(),
        })
    }

    fn ended(&self) -> bool {
        self.lifetime.ended()
    }

    fn sample_memory(self: &Rc<Self>, epoch: u64) {
        let page = Rc::clone(self);
        owner::queue(self, JobKind::Memory, move |runtime, js| {
            let registry = page.host.registry();
            if !registry.memory_is_current(epoch) {
                return;
            }
            let mut reported = registry.note_memory(MemoryItem::scripting_engine(
                MemoryCategory::MainThreadScriptingEngine,
                &js.memory_usage(),
            ));
            let footprint = runtime.with_document(|document| document.memory_footprint());
            reported |= registry.note_memory(MemoryItem::elements(&footprint));
            if let Some(key) = registry.page_background() {
                runtime.send_to_worker(
                    key,
                    crate::background::WorkerMessage::MemorySample {
                        enqueued: registry.js_blocking_stamp(),
                    },
                );
            }
            if reported {
                page.host.wake();
            }
        });
    }

    fn post_content_visibility_changes(self: &Rc<Self>) {
        let page = Rc::clone(self);
        owner::queue(self, JobKind::ContentVisibility, move |runtime, js| {
            page.content_visibility_posted.set(false);
            for failure in runtime.dispatch_content_visibility_changes(js) {
                policy::report(&page, Scene::Listener, failure.into_script_error());
            }
        });
    }

    fn post_intersection_notifications(self: &Rc<Self>) {
        let page = Rc::clone(self);
        owner::queue(self, JobKind::Intersection, move |runtime, js| {
            page.intersections_posted.set(false);
            for failure in runtime.notify_intersection_observers(js) {
                policy::report(&page, Scene::Listener, failure.into_script_error());
            }
        });
    }

    fn post_resize_notifications(self: &Rc<Self>) {
        let page = Rc::clone(self);
        owner::queue(self, JobKind::ResizeObserver, move |runtime, js| {
            page.resizes_posted.set(false);
            for failure in runtime.notify_resize_observers(js) {
                policy::report(&page, Scene::Listener, failure.into_script_error());
            }
        });
    }

    fn post_component_events(self: &Rc<Self>) {
        let page = Rc::clone(self);
        owner::queue(self, JobKind::ComponentEvent, move |runtime, js| {
            page.component_events_posted.set(false);
            for failure in runtime.dispatch_component_events(js) {
                policy::report(&page, Scene::Listener, failure.into_script_error());
            }
        });
    }

    async fn apply(self: &Rc<Self>, commands: Vec<ToMain>) {
        if self.ended() {
            return;
        }
        let count = commands
            .iter()
            .filter(|command| {
                !matches!(
                    command,
                    ToMain::ModuleCallback { .. } | ToMain::CanvasResized
                )
            })
            .count() as u64;
        #[cfg(test)]
        let commands = self.take_test_seams(commands);
        let page = Rc::clone(self);
        owner::enter(self, JobKind::Command, move |runtime, js| {
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
            page.applied.set(page.applied.get() + count);
            if let Some(frame) = page.frame_due.get()
                && frame.fence <= page.applied.get()
                && !page.ended()
            {
                page.frame_due.set(None);
                let pending = page.pending_begin_frame.get().unwrap_or(0);
                page.pending_begin_frame.set(Some(frame.seq.max(pending)));
                runtime.begin_frame(frame.now);
            }
        })
        .await;
    }

    fn adopt_posted(&self, runtime: &mut MainThreadRuntime) {
        let mut scrolled = self.scrolled.borrow_mut();
        if let Some(frame) = self.scroll.take(&mut scrolled) {
            let due = self.frame_due.get();
            self.frame_due
                .set(Some(due.map_or(frame, |due| due.merge(frame))));
        }
        runtime.adopt_scroll_offsets(scrolled.drain());
    }

    fn apply_command(
        self: &Rc<Self>,
        runtime: &mut MainThreadRuntime,
        js: &mut ScriptRuntime,
        command: ToMain,
    ) {
        match command {
            ToMain::PageUpdate(update) => {
                if let Err(error) = runtime.apply_page_update(js, &update) {
                    policy::report(self, Scene::HostCall, error.into_script_error());
                }
            }
            ToMain::DispatchEvent {
                target,
                name,
                payload,
                release,
            } => {
                for error in runtime.deliver_input_event(js, target, name, &payload, release) {
                    policy::report(self, Scene::Listener, error.into_script_error());
                }
            }
            ToMain::Edit(command) => {
                for error in runtime.apply_host_edit(js, command) {
                    policy::report(self, Scene::Listener, error.into_script_error());
                }
            }
            ToMain::TextInputSource(source) => runtime.set_text_input_source(source),
            ToMain::Vsync(milliseconds) => {
                if let Err(error) = runtime.vsync(js, milliseconds) {
                    policy::report(self, Scene::Frame, error.into_script_error());
                }
            }
            ToMain::Posted => self.adopt_posted(runtime),
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
            ToMain::CanvasResized => runtime.with_document(update_placeholders),
            ToMain::FrameLoadMetrics { card, json } => {
                if let Err(error) = runtime.deliver_frame_load_metrics(js, card, &json) {
                    policy::report(self, Scene::HostCall, error.into_script_error());
                }
            }
            #[cfg(feature = "devtools")]
            ToMain::Devtools(attach) => {
                let crate::devtools::Attach {
                    messages,
                    attachment,
                } = *attach;
                let outlet = runtime.devtools_attach(attachment);
                owner::spawn(self, serve_devtools(Rc::clone(self), messages, outlet));
            }
            ToMain::StartMonitorTask(task) => {
                let page = Rc::clone(self);
                owner::spawn(
                    self,
                    task.serve(self.host.clone(), move |epoch| page.sample_memory(epoch)),
                );
            }
            #[cfg(test)]
            ToMain::Probe(probe) => runtime.with_document(probe),
            #[cfg(test)]
            ToMain::Trap(_) => unreachable!("the trap seam is taken before the entry"),
        }
    }

    fn open_realm(self: &Rc<Self>, ingredients: DocumentIngredients, startup: RealmStartup) {
        if self.ended() {
            return;
        }
        let opened = {
            let mut js = self.context.js.borrow_mut();
            match js.as_mut() {
                Err(error) => Some(Err(error.clone())),
                Ok(js) => self.build_realm(js, ingredients, startup),
            }
        };
        match opened {
            None => owner::end(self),
            Some(Err(error)) => policy::report(self, Scene::Open, error),
            Some(Ok(_)) if self.ended() => {}
            Some(Ok((worker_events, checkpoints))) => {
                *self.worker_events.borrow_mut() = Some(worker_events);
                owner::spawn(self, consume_worker_events(Rc::clone(self)));
                owner::spawn(
                    self,
                    serve_clock(Rc::clone(self), self.lifetime.deadlines(), checkpoints),
                );
                let _ = owner::enter_now(self, |_, _| ());
            }
        }
    }

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
        #[cfg(feature = "devtools")]
        let _ = self.console.set(Rc::clone(&runtime.core().console));
        *self.realm.borrow_mut() = Some(Box::new(runtime));
        Some(Ok((worker_events, checkpoints)))
    }

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

    #[cfg(test)]
    fn spawn(self: &Rc<Self>, future: impl std::future::Future<Output = ()> + 'static) {
        owner::spawn(self, future);
    }

    #[cfg(test)]
    fn fail(self: &Rc<Self>, event: EngineEvent) {
        owner::terminal(self, event);
    }

    #[cfg(test)]
    async fn run_owner(self: &Rc<Self>) {
        owner::run_owner(self).await;
    }

    #[cfg(test)]
    pub(super) fn task_count(&self) -> usize {
        self.lifetime.task_count()
    }

    #[cfg(test)]
    pub(super) fn epilogue_count(&self) -> u64 {
        self.lifetime.epilogue_count()
    }

    #[cfg(test)]
    pub(super) fn arm_begin_frame_for_test(&self, seq: u64) {
        self.pending_begin_frame.set(Some(seq));
    }

    #[cfg(test)]
    pub(super) fn armed_deadline(&self) -> Option<ClockInstant> {
        self.lifetime.armed_deadline()
    }
}

impl RealmOwner for Page {
    #[cfg(feature = "devtools")]
    fn console(&self) -> Option<&crate::devtools::runtime::ConsoleTap> {
        self.console.get().map(|console| &**console)
    }

    type Realm = MainThreadRuntime;
    type Event = EngineEvent;
    const THREAD: JsThreadName = JsThreadName::Main;

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

    fn core(runtime: &mut MainThreadRuntime) -> &RealmCore {
        runtime.core()
    }

    fn host(&self) -> &HostOutbox {
        &self.host
    }

    fn send(&self, event: EngineEvent) {
        self.outbox.engine_event(event);
    }

    fn row(scene: Scene) -> Row<EngineEvent> {
        policy::main_thread(scene)
    }

    fn panic_row() -> Row<EngineEvent> {
        policy::main_thread_panic()
    }

    fn before_operation(&self, runtime: &mut MainThreadRuntime) {
        runtime.sync_animation_clock(self.scroll.clock());
    }

    fn run_due_timers(runtime: &mut MainThreadRuntime, js: &mut ScriptRuntime) -> Vec<ScriptError> {
        runtime.run_due_timers(js)
    }

    fn next_deadline(runtime: &mut MainThreadRuntime) -> Option<ClockInstant> {
        runtime.next_deadline()
    }

    fn after_timers(page: &Rc<Self>, runtime: &mut MainThreadRuntime) {
        let owed = runtime.settle_entry();
        if owed.contains(Deliveries::RESIZE) && !page.resizes_posted.replace(true) {
            page.post_resize_notifications();
        }
        if owed.contains(Deliveries::INTERSECTION) && !page.intersections_posted.replace(true) {
            page.post_intersection_notifications();
        }
        if owed.contains(Deliveries::CONTENT_VISIBILITY)
            && !page.content_visibility_posted.replace(true)
        {
            page.post_content_visibility_changes();
        }
        if owed.contains(Deliveries::COMPONENT_EVENTS)
            && !page.component_events_posted.replace(true)
        {
            page.post_component_events();
        }
    }

    fn booted(&self) -> bool {
        self.boot_reported.get()
    }

    fn mark_booted(&self) {
        self.boot_reported.set(true);
    }

    fn on_booted(&self) {
        self.outbox.engine_event(EngineEvent::ScriptFinished);
    }

    fn after_boot(&self) {
        if let Some(seq) = self.pending_begin_frame.take() {
            self.outbox.begin_frame_serviced(seq);
        }
    }

    fn entry_name(&self, runtime: &MainThreadRuntime) -> Option<String> {
        runtime.entry_module_name().ok()
    }

    fn after_settles(page: &Rc<Self>, runtime: &mut MainThreadRuntime) {
        for request in runtime.take_font_face_requests() {
            owner::spawn(page, load_font_face(Rc::clone(page), request));
        }
    }

    fn on_end(&self) {
        let due = self.frame_due.take().map(|frame| frame.seq);
        if let Some(seq) = self.pending_begin_frame.take().max(due) {
            self.outbox.begin_frame_serviced(seq);
        }
    }

    async fn before_release(page: &Rc<Self>) {
        let events = page.worker_events.borrow_mut().take();
        if let Some(mut events) = events
            && let Err(error) = page.dispose(&mut events).await
        {
            page.send(policy::main_thread_disposal().event_for(error.into_script_error()));
        }
    }
}

pub(super) async fn serve_view(context: Rc<GroupContext>, view: AttachedView, outbox: ViewOutbox) {
    let AttachedView {
        viewport,
        sources,
        text_context,
        startup,
        native_modules,
        commands,
        metrics,
        scroll,
        cancel,
    } = view;
    let _cancel = cancel.clone().drop_guard();
    let ViewSources {
        base_url: _,
        config,
        fonts: _,
        default_font_family: _,
        fallback_fonts: _,
        locale: _,
        style_sheets: _,
        entry: _,
        card_url,
        background_entry,
        init_data,
        initial_processor,
        global_props,
        screen,
        #[cfg(feature = "devtools")]
            devtools: _,
    } = sources;
    let ingredients = DocumentIngredients {
        viewport,
        config,
        text_context,
        style_pool: context.style_pool.clone(),
    };
    let StartupSources { sheets, entry } = startup;
    let entry_url = entry.url.clone();
    let page = Page::new(context, outbox, metrics.clone(), scroll, cancel);
    drop(run_job(&page, JobKind::Start, move |page| {
        page.open_realm(
            ingredients,
            RealmStartup {
                sheets,
                screen,
                entry: entry_url,
                card: card_url,
                background_entry,
                initial_processor,
                init_data,
                global_props,
                native_modules,
            },
        );
        Some(())
    }));
    owner::spawn(&page, load_entry(Rc::clone(&page), entry));
    owner::spawn(&page, consume_commands(Rc::clone(&page), commands));
    owner::spawn(&page, consume_metrics(Rc::clone(&page), metrics));
    owner::run_owner(&page).await;
}

fn burst<T, C: FromIterator<T>>(receiver: &mut mpsc::UnboundedReceiver<T>, first: T) -> C {
    let queued = receiver.len();
    let rest = std::iter::from_fn(|| receiver.try_recv().ok()).take(queued);
    std::iter::once(first).chain(rest).collect()
}

async fn consume_commands(page: Rc<Page>, mut commands: mpsc::UnboundedReceiver<ToMain>) {
    while let Some(first) = commands.recv().await {
        if page.outbox.is_cancelled() {
            owner::end(&page);
            return;
        }
        page.apply(burst(&mut commands, first)).await;
    }
    owner::end(&page);
}

async fn consume_metrics(page: Rc<Page>, mut metrics: watch::Receiver<Option<Viewport>>) {
    loop {
        tokio::select! {
            biased;
            () = page.lifetime.token().cancelled() => return,
            changed = metrics.changed() => if changed.is_err() {
                return;
            },
        }
        <Page as Settles>::settle(&page, JobKind::Settle).await;
    }
}

async fn load_entry(page: Rc<Page>, entry: StartupSource) {
    let StartupSource { url, answer } = entry;
    let answered = owner::await_source(answer).await.and_then(|answer| {
        owner::module_answer(&url, answer)
            .and_then(|(response, source)| Ok((absolute_response(&url, response)?, source)))
            .map_err(|reason| LynxViewError::Script(platform_script_error(reason)))
    });
    let completing = Rc::clone(&page);
    owner::enter(&page, JobKind::Entry, move |runtime, js| {
        let completed = answered
            .and_then(|(response_url, source)| runtime.complete_entry(js, &response_url, &source));
        match completed {
            Ok(Ok(())) => {}
            _ if completing.outbox.is_cancelled() => owner::end(&completing),
            Ok(Err(error)) => policy::report(&completing, Scene::Boot, error.into_script_error()),
            Err(error) => owner::terminal(&completing, EngineEvent::StartupFailed(error)),
        }
    })
    .await;
}

fn absolute_response(url: &str, response: String) -> Result<String, String> {
    match url::Url::parse(&response) {
        Ok(_) => Ok(response),
        Err(_) => Err(format!(
            "the fetcher answered {url} from {response:?}, which is not an absolute URL"
        )),
    }
}

async fn load_font_face(page: Rc<Page>, request: terminus::link::FontFaceRequest) {
    let terminus::link::FontFaceRequest { family, sources } = request;
    let mut loaded = None;
    for source in sources {
        let terminus::link::FontFaceSource::Url(url) = source else {
            continue;
        };
        if page.ended() {
            return;
        }
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
    owner::enter(&page, JobKind::FontFace, move |runtime, _| {
        runtime.register_font_face(&family, blob);
    })
    .await;
}

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
        let Some(first) = event else {
            return;
        };
        let events: SmallVec<[WorkerEvent; 1]> = burst(
            page.worker_events
                .borrow_mut()
                .as_mut()
                .expect("the live realm installed its Worker inbox"),
            first,
        );
        let delivering = Rc::clone(&page);
        owner::enter(&page, JobKind::WorkerEvent, move |runtime, js| {
            for WorkerEvent { key, payload } in events {
                if delivering.ended() {
                    break;
                }
                if let Err(error) = runtime.dispatch_worker_event(js, key, payload) {
                    policy::report(&delivering, Scene::Listener, error.into_script_error());
                }
            }
        })
        .await;
    }
}

#[cfg(feature = "devtools")]
async fn serve_devtools(
    page: Rc<Page>,
    mut messages: mpsc::UnboundedReceiver<String>,
    outlet: Arc<crate::devtools::Outlet>,
) {
    while let Some(text) = messages.recv().await {
        let outlet = Arc::clone(&outlet);
        let reporting = Rc::clone(&page);
        owner::queue(&page, JobKind::Devtools, move |runtime, js| {
            if let Err(error) = runtime.devtools_message(js, &outlet, &text) {
                policy::report(&reporting, Scene::Devtools, error);
            }
        });
    }
    owner::queue(&page, JobKind::Devtools, move |runtime, _| {
        runtime.devtools_end(&outlet);
    });
}

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

#[cfg(test)]
#[path = "editing_tests.rs"]
mod editing_tests;
