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
//! `BeginFrame` are *observed state* — a painter wants the latest and never
//! the ones it slept through, which is what [`Published`] on a watch is.

use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use dom::scroll::ScrollAxes;
use dom::{CommittedFrame, FrameImages, HitTarget, NodeId, Vector2D};
use rustc_hash::FxHashSet;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::clock::ClockInstant;
#[cfg(test)]
use crate::main::tree::LynxDocument;
use crate::paint::RouterHost;
use crate::resource::{LoadedSource, SourceCompletion, SourceRequest};
use crate::view::{EngineEvent, EventRequester, LynxViewError};

/// The answer to one source request, as the side that awaits it sees it.
pub(crate) type SourceAnswer = oneshot::Receiver<Result<LoadedSource, LynxViewError>>;

/// Embedder and painter → the view's task: every fact the document must see.
///
/// There is no attach, no shutdown and no source completion among them:
/// attaching is the group's own inbox, the goodbye is this channel closing,
/// and a source answers the one-shot that was minted with its request.
pub(crate) enum ToMain {
    /// Global events accepted after the host observes readiness retain FIFO order.
    PageUpdate(PageUpdate),
    DispatchEvent {
        target: NodeId,
        name: &'static str,
        detail: String,
    },
    Resize {
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    },
    BeginFrame {
        now: f64,
        seq: u64,
    },
    Refill {
        offsets: Vec<(NodeId, Vector2D<f32>)>,
    },
    /// The host's image reports: completed or failed loads. No variant can
    /// carry pixels, which is what makes "`ImageData` never crosses a
    /// channel" a property of the type.
    ImageEvents(Vec<dom::ImageEvent>),
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
    /// The one command that is not the realm's: it spawns a task of the view
    /// that panics on its first poll, and behind it one that answers this
    /// channel with what the view's end latch said when it was next polled.
    /// The page takes it before it enters the realm, so the panic reaches a
    /// sibling and the owner the way any other task's does.
    #[cfg(test)]
    Trap(std::sync::mpsc::Sender<bool>),
}

pub(crate) enum PageUpdate {
    GlobalEvent {
        name: String,
        arguments: Vec<serde_json::Value>,
    },
}

impl PageUpdate {
    pub(crate) fn into_message(self) -> serde_json::Value {
        match self {
            Self::GlobalEvent { name, arguments } => {
                serde_json::json!({"method":"sendGlobalEvent", "name":name, "args":arguments})
            }
        }
    }
}

/// The view's task → the embedder, drained by `LynxView::pump`.
///
/// Only what a host must act on rides here. Everything the painter merely
/// reads is [`Published`] instead.
pub(crate) enum ViewNotice {
    Engine(EngineEvent),
    /// Sources the last paint walk met that the store has not been asked for.
    RequestImages(Vec<Arc<str>>),
    /// One source — a stylesheet, the entry, an imported module or a worker
    /// script — and the right to answer it. Whoever holds the receiving end
    /// is the destination, which is why the host never learns which.
    RequestSource {
        request: SourceRequest,
        completion: SourceCompletion,
    },
}

/// What a view publishes and an observer reads: the latest of each, never a
/// history of any.
#[derive(Clone, Default)]
pub(crate) struct Published {
    /// The newest committed frame, and only ever the newest.
    pub(crate) frame: Option<Arc<CommittedFrame>>,
    /// The event names the realm currently has a listener for anywhere in
    /// the document. Rebuilt on a global edge — a name's first registration
    /// and its last removal — which is rare enough that the whole set is
    /// cheaper than a protocol for the difference.
    pub(crate) listeners: Arc<FxHashSet<Arc<str>>>,
    /// The newest `BeginFrame` the view has serviced.
    pub(crate) begin_frame_serviced: u64,
}

impl Published {
    /// The commit a frame came from, which is the whole of what makes one
    /// frame different from another.
    pub(crate) fn commit(&self) -> Option<u64> {
        self.frame.as_ref().map(|frame| frame.commit_id())
    }
}

/// Every document fact the gesture router asks for is in the snapshot the
/// painter has adopted, so the snapshot *is* the router's host.
///
/// One pass therefore answers out of one state: the frame the pass hit-tested
/// against is the frame a latched scroller is looked up in, and the listener
/// names are the ones that pass adopted.
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

/// The non-owning seat one painter takes on one view: the two things a painter
/// reaches a live view through, released together.
///
/// The view holds the only `Rc`, and [`crate::Painter::attach`] is the only
/// place a view's seat is downgraded (`Painter::detached` builds a seat of its
/// own for the tests that play a view by hand) — so whether a view already has
/// an interactive painter is the weak count of this, rather than a flag
/// somebody has to remember to clear. The painter's `Weak` failing to upgrade
/// is the view being gone, which is a fact the painter reads rather than one it
/// is told.
///
/// The field order is the release order: the goodbye — closing the view's
/// command channel, which is what ends its task — precedes giving up the
/// view's share of the host's resource system.
pub(crate) struct ViewSeat {
    /// The view's own strong sender. Closing it is the goodbye that ends the
    /// view's task, which is why the seat dies with the view rather than with
    /// whatever a painter is holding.
    pub(crate) commands: mpsc::UnboundedSender<ToMain>,
    /// The host's resource system, as the painter reads a commit's pixels out
    /// of it. A clone of the view's own handle, so the store is released when
    /// the view drops both — seat first, by declaration order there.
    pub(crate) images: Rc<dyn FrameImages>,
}

/// The view task's sending end: what the runtime, the tree and the listener
/// index publish through.
///
/// One erased wakeup per group rather than a type parameter threaded through
/// every main-side type: a group's views paint on the thread that created the
/// group, so they wake one event loop, and one virtual call per wake is what
/// that costs.
#[derive(Clone)]
pub(crate) struct ViewOutbox {
    notices: mpsc::UnboundedSender<ViewNotice>,
    /// `Rc` because the sender is the task's and every host closure that
    /// publishes holds a clone of this whole outbox.
    frames: Rc<watch::Sender<Published>>,
    requester: Arc<dyn EventRequester>,
    /// This view's end signal, minted by `create_lynx_view` and cancelled by
    /// the embedder's release, by a fatal lifecycle event, or by the view's
    /// own owner as it exits. Every source completion this outbox hands out
    /// carries a clone, which is what lets a host read cancellation without
    /// waiting for a turn.
    token: CancellationToken,
}

impl ViewOutbox {
    pub(crate) fn new(
        notices: mpsc::UnboundedSender<ViewNotice>,
        frames: watch::Sender<Published>,
        requester: Arc<dyn EventRequester>,
        token: CancellationToken,
    ) -> Self {
        Self {
            notices,
            frames: Rc::new(frames),
            requester,
            token,
        }
    }

    /// This view's end signal, for the realm that mints a child of it per
    /// worker it creates.
    pub(crate) const fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Announces one notice, then wakes the thread that paints.
    ///
    /// Enqueue before requesting a host turn, so that turn's pump observes
    /// the notice.
    pub(crate) fn notify(&self, notice: ViewNotice) {
        if self.notices.send(notice).is_ok() {
            self.requester.request_event();
        }
    }

    pub(crate) fn engine_event(&self, event: EngineEvent) {
        self.notify(ViewNotice::Engine(event));
    }

    /// Asks the host for one source, handing back the answer's receiving end.
    ///
    /// The receiver is the destination and the only one: a module's answer
    /// goes to the task awaiting it, and a worker's rides to the worker
    /// thread inside its `Start`.
    pub(crate) fn request_source(&self, request: SourceRequest) -> SourceAnswer {
        let (completion, answer) = SourceCompletion::new(self.token.clone());
        self.notify(ViewNotice::RequestSource {
            request,
            completion,
        });
        answer
    }

    /// The right to answer one source request that already has a receiver —
    /// a worker script, whose answer travels to `bobcat-workers` rather than
    /// staying here.
    pub(crate) fn completion_for(
        &self,
        answer: oneshot::Sender<Result<LoadedSource, LynxViewError>>,
    ) -> SourceCompletion {
        SourceCompletion::over(answer, self.token.clone())
    }

    /// Publishes the newest committed frame.
    pub(crate) fn publish_frame(&self, frame: Arc<CommittedFrame>) {
        self.frames.send_modify(|published| {
            published.frame = Some(frame);
        });
        self.requester.request_event();
    }

    /// Publishes a global listener-name edge: the first registration for a
    /// name, or the removal of its last.
    ///
    /// No wakeup. Nothing a host does depends on it — the painter reads the
    /// set at the start of its next routing pass, which is a turn the host
    /// was taking anyway.
    pub(crate) fn listener_edge(&self, name: Arc<str>, available: bool) {
        self.frames.send_modify(|published| {
            let mut names = published.listeners.as_ref().clone();
            if available {
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

    /// Whether this view is over. A mutex read on the token, so it is asked
    /// where a turn would otherwise be waited for — never inside an entry,
    /// which reads the thread-local latch instead.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

/// What a view publishes, as a reader that is not a painter sees it.
///
/// One adopted snapshot plus the watch it came from, which is exactly what
/// the listener index is read through. The crate's benchmarks and the tests
/// that drive a document in place hold one; a painter keeps the two halves
/// apart instead, because its snapshot has to outlive the view it detached
/// from.
pub(crate) struct ViewObserver {
    frames: watch::Receiver<Published>,
    published: Published,
}

impl ViewObserver {
    /// Adopts the newest published state, and says whether it had moved.
    ///
    /// Never `Receiver::has_changed()`: that reports an error once the sender
    /// is gone, and what a view published before its task ended is still the
    /// last true answer.
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

    /// Every listener name the view has published, for a test that asserts
    /// the whole set rather than one membership.
    #[cfg(test)]
    pub(crate) fn listener_names(&self) -> Vec<Arc<str>> {
        self.published.listeners.iter().cloned().collect()
    }

    /// Whether the published state moved since this was last asked, which
    /// for a test driving the listener index alone is whether an edge
    /// crossed.
    #[cfg(test)]
    pub(crate) fn take_published_edge(&mut self) -> bool {
        self.adopt()
    }

    /// The commit the newest published frame came from, for a test that is
    /// itself the far end of the view it is watching.
    #[cfg(test)]
    pub(crate) fn commit(&mut self) -> Option<u64> {
        self.sync();
        self.published.commit()
    }

    /// The newest `BeginFrame` the view has acknowledged.
    #[cfg(test)]
    pub(crate) fn begin_frame_serviced(&mut self) -> u64 {
        self.sync();
        self.published.begin_frame_serviced
    }
}

/// The far end of one view's link, for a caller that is itself that end: the
/// crate's benchmarks, and the tests that drive a document in place rather
/// than over a group's thread.
pub(crate) struct DetachedView {
    /// Held even where nothing reads it: a closed notice channel would make
    /// the outbox's sends fail, which is not the shape a caller playing the
    /// host is standing in for.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "held open for the outbox; read by the crate's tests"
        )
    )]
    pub(crate) notices: mpsc::UnboundedReceiver<ViewNotice>,
    pub(crate) published: ViewObserver,
    /// The end signal every completion this view hands out carries. A caller
    /// playing the host is the one that cancels it, since there is no
    /// `LynxView` here to be dropped.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the token every completion carries; spelled by the crate's tests"
        )
    )]
    pub(crate) token: CancellationToken,
}

/// One view's publishing end and the far end that reads it, with no thread
/// between them.
pub(crate) fn detached_outbox(requester: Arc<dyn EventRequester>) -> (ViewOutbox, DetachedView) {
    let token = CancellationToken::new();
    let (notices, notice_receiver) = mpsc::unbounded_channel();
    let (frames, frame_receiver) = watch::channel(Published::default());
    (
        ViewOutbox::new(notices, frames, requester, token.clone()),
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

/// Polls `future` on this thread until it is ready or `deadline` passes.
///
/// The one place a host's own thread blocks on `bobcat-main`, reached from
/// `tick` and from the crate's tests. It parks rather than spins, and it
/// takes the clock explicitly because `std::time` is not available on every
/// target this engine runs on.
///
/// The future is polled *unconstrained*: tokio's cooperative budget would
/// otherwise stop yielding to it after a few hundred ready polls, and there
/// is no runtime here to hand the budget back.
pub(crate) fn block_on_deadline<F: Future>(future: F, deadline: ClockInstant) -> Option<F::Output> {
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
