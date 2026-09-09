//! One view's link: the two directions it crosses, the state a painter
//! observes without asking, and the flag that ends everything on it.
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use dom::{CommittedFrame, NodeId, Vector2D};
use rustc_hash::FxHashSet;
use tokio::sync::{mpsc, oneshot, watch};

use crate::clock::ClockInstant;
#[cfg(test)]
use crate::main::tree::LynxDocument;
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

/// One view's "this is over", readable without waiting for anything.
///
/// The channels already say it — a closed command channel is a released view
/// — but they say it *eventually*, on the receiving thread's next turn. A
/// host that drops a view and immediately asks whether the load it is holding
/// still matters needs the answer now, and a source completion is the one
/// thing outside this engine that asks.
///
/// Cancellation is cooperative: an IO operation already running may finish,
/// and synchronous JavaScript already executing runs to its end. What it
/// guarantees is that no result crosses afterwards.
#[derive(Clone, Debug, Default)]
pub(crate) struct ViewCancel(Arc<AtomicBool>);

impl ViewCancel {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Sets the flag on every exit from the scope it guards, including a panic.
pub(crate) struct CancelOnExit(pub(crate) ViewCancel);

impl Drop for CancelOnExit {
    fn drop(&mut self) {
        self.0.cancel();
    }
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
    cancel: ViewCancel,
}

impl ViewOutbox {
    pub(crate) fn new(
        notices: mpsc::UnboundedSender<ViewNotice>,
        frames: watch::Sender<Published>,
        requester: Arc<dyn EventRequester>,
        cancel: ViewCancel,
    ) -> Self {
        Self {
            notices,
            frames: Rc::new(frames),
            requester,
            cancel,
        }
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
        let (completion, answer) = SourceCompletion::new(self.cancel.clone());
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
        SourceCompletion::over(answer, self.cancel.clone())
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

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// What a view publishes, as a reader that is not a painter sees it.
///
/// One adopted snapshot plus the watch it came from. The crate's tests stand
/// in for the far end of a view's link and hold one; a painter keeps the two
/// halves apart instead, because its snapshot has to outlive the view it is
/// drawing for.
#[cfg(test)]
pub(crate) struct ViewObserver {
    frames: watch::Receiver<Published>,
    published: Published,
}

#[cfg(test)]
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

    /// The commit the newest published frame came from, for a test that is
    /// itself the far end of the view it is watching.
    pub(crate) fn commit(&mut self) -> Option<u64> {
        self.sync();
        self.published.commit()
    }

    /// The newest `BeginFrame` the view has acknowledged.
    pub(crate) fn begin_frame_serviced(&mut self) -> u64 {
        self.sync();
        self.published.begin_frame_serviced
    }
}

/// The far end of one view's link, for a caller that is itself that end: the
/// tests that drive a view in place rather than over a group's thread.
#[cfg(test)]
pub(crate) struct DetachedView {
    /// Held even where nothing reads it: a closed notice channel would make
    /// the outbox's sends fail, which is not the shape a caller playing the
    /// host is standing in for.
    pub(crate) notices: mpsc::UnboundedReceiver<ViewNotice>,
    pub(crate) published: ViewObserver,
    pub(crate) cancel: ViewCancel,
}

/// One view's publishing end and the far end that reads it, with no thread
/// between them.
#[cfg(test)]
pub(crate) fn detached_outbox(requester: Arc<dyn EventRequester>) -> (ViewOutbox, DetachedView) {
    let cancel = ViewCancel::default();
    let (notices, notice_receiver) = mpsc::unbounded_channel();
    let (frames, frame_receiver) = watch::channel(Published::default());
    (
        ViewOutbox::new(notices, frames, requester, cancel.clone()),
        DetachedView {
            notices: notice_receiver,
            published: ViewObserver {
                frames: frame_receiver,
                published: Published::default(),
            },
            cancel,
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
