//! One view's link: the two directions it crosses, the state a painter
//! observes without asking, and the token that ends everything on it.
//!
//! A view owns its channels end to end. Nothing here is addressed, because
//! there is nobody else on the wire: the embedder's thread holds one sending
//! end and one receiving end per view, and the task serving that view on
//! `bobcat-main` holds the other two. A sibling's traffic is not on this path
//! at all, so no message names its view and no receiver has to defer one.
//!
//! The two directions are deliberately different shapes. Commands are a FIFO,
//! because the order two of them arrive in is what they mean. What comes back
//! is split: lifecycle events and resource asks are a FIFO for the same
//! reason, while the frame, the listener names and the newest serviced
//! frame post are *observed state* — a painter wants the latest and never
//! the ones it slept through, which is what [`Published`] on a watch is.
//!
//! The painter's own state crosses the same way in the other direction. Its
//! scroll offsets and its frame requests are posted into the view's
//! [`ScrollMailbox`], which keeps the latest per scroll container and one
//! coalesced frame post; one payload-free [`ToMain::Posted`] marker in the
//! command FIFO says there is something to take, and at most one is ever
//! queued. So a main thread inside a long job accumulates nothing, and the
//! painter never waits for it. A frame post is fenced by the count of
//! commands sent before it ([`CommandSender`]), so main applies it behind
//! them wherever the marker stands. The painter's clock rides beside them,
//! read lock-free at the start of every job.
//!
//! The painter's device metrics are observed state too, and
//! [`ViewSeat::metrics`] is the watch that carries them. They are not a
//! command: a flush before any painter has bound parks the job it runs in,
//! and no other job runs while one is parked, so a command carrying them
//! would never be read.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::Entry;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rustc_hash::{FxHashMap, FxHashSet};
use terminus::link::painter::RouterHost;
pub(crate) use terminus::link::painter::ScrollEntry;
use terminus::link::scroll::ScrollAxes;
use terminus::link::{CommittedFrame, FrameImages, HitTarget, NodeId};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::background::{WorkerKey, WorkerMessage};
use crate::canvas::CanvasHandOver;
#[cfg(test)]
use crate::main::tree::LynxDocument;
use crate::native_module::{ModuleReply, NativeModuleEsms};
use crate::paint::CanvasPictures;
use crate::performance::PipelineRegistry;
use crate::resource::{FetchProbe, LoadedSource, SourceCompletion, SourceRequest, StyleSheetProbe};
use crate::view::{EngineEvent, EventRequester, LynxViewError, Viewport, WorkerFonts};

pub(crate) type SourceAnswer = oneshot::Receiver<Result<LoadedSource, LynxViewError>>;

#[derive(Clone)]
pub(crate) struct HostOutbox {
    notices: mpsc::UnboundedSender<ViewNotice>,
    requester: Arc<dyn EventRequester>,
    token: CancellationToken,
    probe: Option<FetchProbe>,
    base: Arc<Url>,
    registry: Arc<PipelineRegistry>,
    native_esms: Arc<NativeModuleEsms>,
    canvas_hand_over: Arc<CanvasHandOver>,
    worker_fonts: Arc<WorkerFonts>,
}

impl HostOutbox {
    pub(crate) fn new(
        notices: mpsc::UnboundedSender<ViewNotice>,
        requester: Arc<dyn EventRequester>,
        token: CancellationToken,
        probe: Option<FetchProbe>,
        base: Arc<Url>,
        registry: Arc<PipelineRegistry>,
    ) -> Self {
        Self {
            notices,
            requester,
            token,
            probe,
            base,
            registry,
            native_esms: Arc::default(),
            canvas_hand_over: Arc::default(),
            worker_fonts: Arc::default(),
        }
    }

    pub(crate) fn with_native_esms(mut self, native_esms: Arc<NativeModuleEsms>) -> Self {
        self.native_esms = native_esms;
        self
    }

    pub(crate) fn with_canvas_hand_over(mut self, store: Arc<CanvasHandOver>) -> Self {
        self.canvas_hand_over = store;
        self
    }

    pub(crate) const fn canvas_hand_over(&self) -> &Arc<CanvasHandOver> {
        &self.canvas_hand_over
    }

    pub(crate) fn with_worker_fonts(mut self, worker_fonts: Arc<WorkerFonts>) -> Self {
        self.worker_fonts = worker_fonts;
        self
    }

    pub(crate) const fn worker_fonts(&self) -> &Arc<WorkerFonts> {
        &self.worker_fonts
    }

    pub(crate) fn native_esms(&self) -> &NativeModuleEsms {
        &self.native_esms
    }

    pub(crate) const fn registry(&self) -> &Arc<PipelineRegistry> {
        &self.registry
    }

    pub(crate) fn wake(&self) {
        self.requester.request_event();
    }

    pub(crate) fn resolve(&self, url: &str) -> Result<String, String> {
        self.base
            .join(url)
            .map(String::from)
            .map_err(|error| format!("cannot resolve `{url}` against `{}`: {error}", self.base))
    }

    pub(crate) const fn fetch_probe(&self) -> Option<&FetchProbe> {
        self.probe.as_ref()
    }

    pub(crate) const fn token(&self) -> &CancellationToken {
        &self.token
    }

    pub(crate) fn request(&self, request: SourceRequest) -> SourceAnswer {
        let (completion, answer) = SourceCompletion::new(self.token.clone());
        self.send(request, completion);
        answer
    }

    pub(crate) fn notify(&self, notice: ViewNotice) {
        if self.notices.send(notice).is_ok() {
            self.requester.request_event();
        }
    }

    pub(crate) fn preload(&self, request: SourceRequest) {
        self.notify(ViewNotice::PreloadSource(request));
    }

    pub(crate) fn engine_event(&self, event: EngineEvent) {
        self.notify(ViewNotice::Engine(event));
    }

    fn send(&self, request: SourceRequest, completion: SourceCompletion) {
        self.notify(ViewNotice::RequestSource {
            request,
            completion,
        });
    }
}

pub(crate) enum ToMain {
    PageUpdate(PageUpdate),
    DispatchEvent {
        target: NodeId,
        name: &'static str,
        payload: InputEventPayload,
        release: Option<NodeId>,
    },
    Vsync(f64),
    Edit(terminus::link::editing::Command),
    TextInputSource(crate::editing::TextInputSource),
    Posted,
    ImageEvents(Vec<terminus::link::ImageEvent>),
    ModuleCallback {
        call: u64,
        index: u32,
        arguments: Option<String>,
    },
    CanvasResized,
    StartMonitorTask(crate::performance::MonitorTask),
    FrameLoadMetrics {
        card: crate::performance::OpenerId,
        json: String,
    },
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
    #[cfg(test)]
    Trap(std::sync::mpsc::Sender<bool>),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InputEventPayload {
    pub(crate) detail: terminus::link::event::EventDetail,
    pub(crate) timestamp: f64,
}

pub(crate) enum PageUpdate {
    Reload {
        data: String,
        processor_name: String,
    },
    Data {
        data: String,
        processor_name: String,
        reset: bool,
    },
    GlobalProps(String),
    GlobalEvent {
        name: String,
        arguments: String,
    },
}

pub(crate) enum ViewNotice {
    WorkerCreated {
        key: WorkerKey,
        messages: mpsc::WeakUnboundedSender<WorkerMessage>,
    },
    ScriptFrameDemand {
        worker: Option<WorkerKey>,
        pending: bool,
    },
    Engine(EngineEvent),
    PreloadSource(SourceRequest),
    RequestImages(Vec<Arc<str>>),
    RequestDocuments(Vec<terminus::link::DocumentRequest>),
    Playback(Vec<terminus::link::PlaybackRequest>),
    RequestSource {
        request: SourceRequest,
        completion: SourceCompletion,
    },
    TextInputState(Box<terminus::link::editing::TextInputState>),
    NativeModuleCall {
        caller: Option<WorkerKey>,
        call: u64,
        module: String,
        method: String,
        arguments: String,
        callbacks: Vec<u32>,
    },
}

#[derive(Clone, Default)]
pub(crate) struct Published {
    pub(crate) frame: Option<Arc<CommittedFrame>>,
    pub(crate) listeners: Arc<FxHashSet<Arc<str>>>,
    pub(crate) begin_frame_serviced: u64,
}

impl Published {
    #[cfg(test)]
    pub(crate) fn commit(&self) -> Option<u64> {
        self.frame.as_ref().map(|frame| frame.commit_id())
    }
}

impl RouterHost for Published {
    fn nearest_user_scrollable(&self, from: HitTarget, axes: ScrollAxes) -> Option<NodeId> {
        let frame = self.frame.as_ref()?;
        let slot = frame.nearest_user_scrollable(from.scroll, axes)?;
        Some(frame.scroll_slots()[slot as usize].node)
    }

    fn contains_node(&self, node: NodeId) -> bool {
        self.frame
            .as_ref()
            .is_some_and(|frame| frame.slot_of(node).is_some())
    }

    fn has_listener(&self, name: &str) -> bool {
        self.listeners.contains(name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FramePost {
    pub(crate) now: f64,
    pub(crate) seq: u64,
    pub(crate) fence: u64,
}

impl FramePost {
    #[must_use]
    pub(crate) fn merge(self, later: Self) -> Self {
        Self {
            now: later.now,
            seq: self.seq.max(later.seq),
            fence: self.fence.max(later.fence),
        }
    }
}

#[derive(Default)]
pub(crate) struct ScrollMailbox {
    posted: Mutex<Posted>,
    clock: AtomicU64,
}

#[derive(Default)]
struct Posted {
    scrolled: FxHashMap<NodeId, ScrollEntry>,
    frame: Option<FramePost>,
    armed: bool,
}

impl ScrollMailbox {
    pub(crate) fn post(
        &self,
        entries: impl IntoIterator<Item = (NodeId, ScrollEntry)>,
        frame: Option<FramePost>,
    ) -> bool {
        let mut posted = lock(&self.posted);
        for (node, entry) in entries {
            match posted.scrolled.entry(node) {
                Entry::Occupied(mut held) => {
                    let rest = entry.rest.or(held.get().rest);
                    *held.get_mut() = ScrollEntry { rest, ..entry };
                }
                Entry::Vacant(vacant) => {
                    vacant.insert(entry);
                }
            }
        }
        if let Some(frame) = frame {
            posted.frame = Some(posted.frame.map_or(frame, |held| held.merge(frame)));
        }
        !std::mem::replace(&mut posted.armed, true)
    }

    pub(crate) fn take(&self, spare: &mut FxHashMap<NodeId, ScrollEntry>) -> Option<FramePost> {
        debug_assert!(spare.is_empty(), "the last take was spent");
        let mut posted = lock(&self.posted);
        std::mem::swap(&mut posted.scrolled, spare);
        posted.armed = false;
        posted.frame.take()
    }

    pub(crate) fn set_clock(&self, now: f64) {
        self.clock.store(now.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn clock(&self) -> f64 {
        f64::from_bits(self.clock.load(Ordering::Relaxed))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|error| panic!("a view's scroll mailbox is poisoned: {error}"))
}

pub(crate) struct CommandSender {
    sender: mpsc::UnboundedSender<ToMain>,
    sent: Cell<u64>,
}

impl CommandSender {
    pub(crate) const fn new(sender: mpsc::UnboundedSender<ToMain>) -> Self {
        Self {
            sender,
            sent: Cell::new(0),
        }
    }

    pub(crate) fn send(&self, command: ToMain) -> Result<(), mpsc::error::SendError<ToMain>> {
        self.sent.set(self.sent.get() + 1);
        self.sender.send(command)
    }

    pub(crate) fn sent(&self) -> u64 {
        self.sent.get()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    pub(crate) fn uncounted(&self) -> mpsc::WeakUnboundedSender<ToMain> {
        self.sender.downgrade()
    }
}

#[derive(Default)]
pub(crate) struct FrameDemand {
    main: bool,
    workers: FxHashMap<WorkerKey, (mpsc::WeakUnboundedSender<WorkerMessage>, bool)>,
    registry: Option<Arc<PipelineRegistry>>,
}

impl FrameDemand {
    pub(crate) fn new(registry: Arc<PipelineRegistry>) -> Self {
        Self {
            registry: Some(registry),
            ..Self::default()
        }
    }

    pub(crate) fn register_worker(
        &mut self,
        key: WorkerKey,
        messages: mpsc::WeakUnboundedSender<WorkerMessage>,
    ) {
        self.workers.insert(key, (messages, false));
    }

    pub(crate) fn reply(
        &self,
        caller: Option<WorkerKey>,
        main: &CommandSender,
    ) -> Option<ModuleReply> {
        match caller {
            Some(key) => self
                .workers
                .get(&key)
                .map(|(messages, _)| ModuleReply::Worker(messages.clone(), self.registry.clone())),
            None => Some(ModuleReply::Main(main.uncounted())),
        }
    }

    pub(crate) fn set(&mut self, worker: Option<WorkerKey>, pending: bool) {
        if let Some(key) = worker {
            if let Some((_, demand)) = self.workers.get_mut(&key) {
                *demand = pending;
            }
        } else {
            self.main = pending;
        }
    }

    pub(crate) fn is_pending(&mut self) -> bool {
        self.workers.retain(|_, (messages, _)| {
            messages.upgrade().is_some_and(|sender| !sender.is_closed())
        });
        self.main || self.workers.values().any(|(_, pending)| *pending)
    }

    pub(crate) fn notify_performance(&self, key: WorkerKey) {
        if let Some((messages, _)) = self.workers.get(&key)
            && let Some(messages) = messages.upgrade()
        {
            let enqueued = self
                .registry
                .as_deref()
                .and_then(PipelineRegistry::js_blocking_stamp);
            let _ = messages.send(WorkerMessage::PerformanceReady { enqueued });
        }
    }

    pub(crate) fn dispatch(&mut self, milliseconds: f64, main: &CommandSender) {
        if std::mem::take(&mut self.main) {
            let _ = main.send(ToMain::Vsync(milliseconds));
        }
        let registry = self.registry.as_deref();
        for (messages, pending) in self.workers.values_mut() {
            if std::mem::take(pending)
                && let Some(messages) = messages.upgrade()
            {
                let _ = messages.send(WorkerMessage::Vsync {
                    milliseconds,
                    enqueued: registry.and_then(PipelineRegistry::js_blocking_stamp),
                });
            }
        }
    }
}

pub(crate) struct ViewSeat<P = std::convert::Infallible> {
    pub(crate) frame_demand: RefCell<FrameDemand>,
    pub(crate) commands: CommandSender,
    pub(crate) metrics: watch::Sender<Option<Viewport>>,
    pub(crate) images: Rc<dyn FrameImages<PlatformFrame = P>>,
    pub(crate) canvases: Arc<CanvasPictures>,
    pub(crate) scroll: Arc<ScrollMailbox>,
    pub(crate) registry: Arc<PipelineRegistry>,
}

#[derive(Clone)]
pub(crate) struct ViewOutbox {
    notices: mpsc::UnboundedSender<ViewNotice>,
    frames: Rc<watch::Sender<Published>>,
    requester: Arc<dyn EventRequester>,
    token: CancellationToken,
    probe: Option<FetchProbe>,
    style_sheet_probe: Option<StyleSheetProbe>,
    base: Arc<Url>,
    registry: Arc<PipelineRegistry>,
    native_esms: Arc<NativeModuleEsms>,
    worker_fonts: Arc<WorkerFonts>,
    canvases: Arc<CanvasPictures>,
    main: Option<mpsc::WeakUnboundedSender<ToMain>>,
    canvas_hand_over: Arc<CanvasHandOver>,
}

impl ViewOutbox {
    #[expect(
        clippy::too_many_arguments,
        reason = "one per part of the view's link, each named at its one construction"
    )]
    pub(crate) fn new(
        notices: mpsc::UnboundedSender<ViewNotice>,
        frames: watch::Sender<Published>,
        requester: Arc<dyn EventRequester>,
        token: CancellationToken,
        probe: Option<FetchProbe>,
        style_sheet_probe: Option<StyleSheetProbe>,
        base: Arc<Url>,
        registry: Arc<PipelineRegistry>,
    ) -> Self {
        Self {
            notices,
            frames: Rc::new(frames),
            requester,
            token,
            probe,
            style_sheet_probe,
            base,
            registry,
            native_esms: Arc::default(),
            worker_fonts: Arc::default(),
            canvases: Arc::default(),
            main: None,
            canvas_hand_over: Arc::default(),
        }
    }

    pub(crate) fn with_native_esms(mut self, native_esms: Arc<NativeModuleEsms>) -> Self {
        self.native_esms = native_esms;
        self
    }

    pub(crate) fn with_worker_fonts(mut self, worker_fonts: Arc<WorkerFonts>) -> Self {
        self.worker_fonts = worker_fonts;
        self
    }

    pub(crate) fn with_canvases(
        mut self,
        canvases: Arc<CanvasPictures>,
        main: mpsc::WeakUnboundedSender<ToMain>,
    ) -> Self {
        self.canvases = canvases;
        self.main = Some(main);
        self
    }

    pub(crate) const fn canvases(&self) -> &Arc<CanvasPictures> {
        &self.canvases
    }

    pub(crate) const fn main(&self) -> Option<&mpsc::WeakUnboundedSender<ToMain>> {
        self.main.as_ref()
    }

    pub(crate) const fn requester(&self) -> &Arc<dyn EventRequester> {
        &self.requester
    }

    pub(crate) const fn registry(&self) -> &Arc<PipelineRegistry> {
        &self.registry
    }

    pub(crate) fn wake(&self) {
        self.requester.request_event();
    }

    pub(crate) const fn style_sheet_probe(&self) -> Option<&StyleSheetProbe> {
        self.style_sheet_probe.as_ref()
    }

    pub(crate) const fn token(&self) -> &CancellationToken {
        &self.token
    }

    pub(crate) fn host_outbox(&self, token: CancellationToken) -> HostOutbox {
        HostOutbox::new(
            self.notices.clone(),
            Arc::clone(&self.requester),
            token,
            self.probe.clone(),
            Arc::clone(&self.base),
            Arc::clone(&self.registry),
        )
        .with_native_esms(Arc::clone(&self.native_esms))
        .with_canvas_hand_over(Arc::clone(&self.canvas_hand_over))
        .with_worker_fonts(Arc::clone(&self.worker_fonts))
    }

    pub(crate) fn notify(&self, notice: ViewNotice) {
        if self.notices.send(notice).is_ok() {
            self.requester.request_event();
        }
    }

    pub(crate) fn engine_event(&self, event: EngineEvent) {
        self.notify(ViewNotice::Engine(event));
    }

    pub(crate) fn request_source(&self, request: SourceRequest) -> SourceAnswer {
        let (completion, answer) = SourceCompletion::new(self.token.clone());
        self.notify(ViewNotice::RequestSource {
            request,
            completion,
        });
        answer
    }

    pub(crate) fn publish_frame(&self, frame: Arc<CommittedFrame>) {
        self.frames.send_modify(|published| {
            published.frame = Some(frame);
        });
        self.requester.request_event();
    }

    pub(crate) fn set_name_routed(&self, name: Arc<str>, routed: bool) {
        self.frames.send_modify(|published| {
            let mut names = published.listeners.as_ref().clone();
            if routed {
                names.insert(name);
            } else {
                names.remove(&name);
            }
            published.listeners = Arc::new(names);
        });
    }

    pub(crate) fn begin_frame_serviced(&self, seq: u64) {
        self.frames.send_modify(|published| {
            published.begin_frame_serviced = published.begin_frame_serviced.max(seq);
        });
        self.requester.request_event();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

pub(crate) struct ViewObserver {
    frames: watch::Receiver<Published>,
    published: Published,
}

impl ViewObserver {
    fn adopt(&mut self) -> bool {
        let latest = self.frames.borrow_and_update();
        let changed = latest.has_changed();
        let published = latest.clone();
        drop(latest);
        self.published = published;
        changed
    }

    pub(crate) fn sync(&mut self) {
        let _ = self.adopt();
    }

    pub(crate) fn has_listener(&self, name: &str) -> bool {
        self.published.listeners.contains(name)
    }

    #[cfg(test)]
    pub(crate) fn listener_names(&self) -> Vec<Arc<str>> {
        self.published.listeners.iter().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn commit(&mut self) -> Option<u64> {
        self.sync();
        self.published.commit()
    }

    #[cfg(test)]
    pub(crate) fn frame(&mut self) -> Option<Arc<CommittedFrame>> {
        self.sync();
        self.published.frame.clone()
    }

    #[cfg(test)]
    pub(crate) fn begin_frame_serviced(&mut self) -> u64 {
        self.sync();
        self.published.begin_frame_serviced
    }
}

pub(crate) struct DetachedView {
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "held open for the outbox; read by the crate's tests"
        )
    )]
    pub(crate) notices: mpsc::UnboundedReceiver<ViewNotice>,
    pub(crate) published: ViewObserver,
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the token every completion carries; spelled by the crate's tests"
        )
    )]
    pub(crate) token: CancellationToken,
}

pub(crate) fn detached_base() -> Arc<Url> {
    Arc::new(Url::parse("app:///").expect("`app:///` is an absolute URL"))
}

pub(crate) fn detached_outbox(requester: Arc<dyn EventRequester>) -> (ViewOutbox, DetachedView) {
    detached_outbox_probing_style_sheets(requester, None)
}

pub(crate) fn detached_outbox_probing_style_sheets(
    requester: Arc<dyn EventRequester>,
    style_sheet_probe: Option<StyleSheetProbe>,
) -> (ViewOutbox, DetachedView) {
    let token = CancellationToken::new();
    let (notices, notice_receiver) = mpsc::unbounded_channel();
    let (frames, frame_receiver) = watch::channel(Published::default());
    (
        ViewOutbox::new(
            notices,
            frames,
            requester,
            token.clone(),
            None,
            style_sheet_probe,
            detached_base(),
            PipelineRegistry::detached(),
        ),
        DetachedView {
            notices: notice_receiver,
            published: ViewObserver {
                frames: frame_receiver,
                published: Published::default(),
            },
            token,
        },
    )
}

#[cfg(test)]
pub(crate) use deadline::block_on_deadline;

#[cfg(test)]
mod deadline {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread;

    use crate::clock::ClockInstant;

    pub(crate) fn block_on_deadline<F: Future>(
        future: F,
        deadline: ClockInstant,
    ) -> Option<F::Output> {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(tokio::task::coop::unconstrained(future));
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return Some(output);
            }
            let remaining = deadline.checked_duration_since(ClockInstant::now())?;
            thread::park_timeout(remaining);
        }
    }

    struct Unpark(thread::Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
}
