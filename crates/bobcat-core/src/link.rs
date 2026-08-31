//! The one link each `LynxView` has between its two threads.
//!
//! Two FIFOs are the whole protocol: [`ToMain`] carries every host fact the
//! document must see, [`ToPresenter`] carries everything the main thread has
//! to say back. Both are ordered, so a command and the notification it
//! produces never overtake each other.
//!
//! Frames are the one thing that does not ride a FIFO. A queue of them would
//! retain every intermediate scene the main thread ever built, so a commit
//! goes into a one-slot mailbox that overwrites whatever was there and is
//! announced by a [`ToPresenter::FrameChanged`]. However many commits land
//! between two presenting turns, the presenting side reads one frame — the
//! newest — and however busy the main thread is, the frames in flight cost
//! one.
//!
//! A channel stores a message but cannot wake `AppKit`, winit, or a browser
//! Worker, so the link also holds the embedder's [`EventRequester`] and wakes
//! it *after* — never before — the state it announces is in place. One view
//! has one requester: both ends are generic over the platform's own type, so
//! a wake is a direct call, and the link is the only thing that holds it.

use std::cell::Cell;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use dom::{CommittedFrame, NodeId, Vector2D};
use rustc_hash::FxHashSet;

#[cfg(test)]
use crate::tree::LynxDocument;
use crate::view::{EngineEvent, EventRequester};

/// presenting → Lynx main: every host fact the document must see.
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
    NoteImagesChanged,
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
}

/// Lynx main → presenting: everything the main thread has to say back.
#[derive(Debug)]
pub(crate) enum ToPresenter {
    /// The frame mailbox holds something the presenting side has not read.
    FrameChanged,
    /// A lifecycle event, waiting for the embedder's next `pump`.
    Engine(EngineEvent),
    /// The first listener for a name was registered anywhere in the document.
    ListenerAvailable(Arc<str>),
    /// The last listener for a name went away.
    ListenerUnavailable(Arc<str>),
    /// Every `BeginFrame` through this sequence number has been serviced.
    BeginFrameServiced(u64),
}

/// The latest committed frame, and only ever the latest.
type FrameHub = Mutex<Option<Arc<CommittedFrame>>>;

fn slot(hub: &FrameHub) -> MutexGuard<'_, Option<Arc<CommittedFrame>>> {
    hub.lock()
        .unwrap_or_else(|error| panic!("the frame mailbox is poisoned: {error}"))
}

/// The Lynx main thread's sending end: the notification FIFO, the frame
/// mailbox, and the host wakeup that announces both.
///
/// Cloned rather than shared by reference because the thread's senders are
/// different stack frames — a commit, a listener edge, and a lifecycle event
/// have no call site in common.
pub(crate) struct ToPresenterSender<R: EventRequester> {
    notifications: mpsc::Sender<ToPresenter>,
    frames: Arc<FrameHub>,
    requester: Arc<R>,
}

// Hand-written: `derive(Clone)` would demand `R: Clone`, and a requester is
// shared through its `Arc`, never cloned itself.
impl<R: EventRequester> Clone for ToPresenterSender<R> {
    fn clone(&self) -> Self {
        Self {
            notifications: self.notifications.clone(),
            frames: Arc::clone(&self.frames),
            requester: Arc::clone(&self.requester),
        }
    }
}

impl<R: EventRequester> ToPresenterSender<R> {
    /// Announces one notification, then wakes the host event loop.
    ///
    /// Always in that order: the wake must never overtake the state it
    /// announces. A closed channel is the view going away, which this thread
    /// learns about through its command channel instead.
    pub(crate) fn send(&self, notification: ToPresenter) {
        if self.notifications.send(notification).is_ok() {
            self.requester.request_event();
        }
    }

    /// Publishes a committed frame — into the mailbox, over whatever the
    /// presenting side has not read yet, then announced like anything else.
    pub(crate) fn publish_frame(&self, frame: Arc<CommittedFrame>) {
        *slot(&self.frames) = Some(frame);
        self.send(ToPresenter::FrameChanged);
    }
}

/// The Lynx main thread's end of the link.
pub(crate) struct MainLink<R: EventRequester> {
    /// Commands from the presenting side, in the order it sent them.
    pub(crate) commands: mpsc::Receiver<ToMain>,
    /// Everything this thread has to say back.
    pub(crate) notify: ToPresenterSender<R>,
}

/// The presenting thread's end of the link, and the replicas it keeps of
/// what the main thread has published.
pub(crate) struct PresenterLink<R: EventRequester> {
    commands: mpsc::Sender<ToMain>,
    notifications: mpsc::Receiver<ToPresenter>,
    frames: Arc<FrameHub>,
    requester: Arc<R>,
    /// The newest published frame. Read out of the mailbox only when a sync
    /// finds one announced, so a pass that composes, hit-tests, and refills
    /// against it takes no lock at all.
    frame: Option<Arc<CommittedFrame>>,
    /// Lifecycle events drained from the FIFO, waiting for the next `pump`.
    events: Vec<EngineEvent>,
    /// Lock-free replica of the names the realm has listeners for. It moves
    /// at sync boundaries, which is the one pass of staleness this design
    /// accepts in exchange for the lock.
    listener_names: FxHashSet<Arc<str>>,
    /// `BeginFrame`s sent, and the highest one the main thread has serviced.
    begin_frames_sent: u64,
    begin_frames_serviced: u64,
    /// A frame is owed to the draw target: a new commit, or a request the
    /// presenting side made of itself — a refresh, or a swap chain that
    /// asked to be retried.
    redraw_pending: Cell<bool>,
}

/// Builds one view's link: the presenting end, and the end its Lynx main
/// thread is started over.
pub(crate) fn link<R: EventRequester>(requester: Arc<R>) -> (PresenterLink<R>, MainLink<R>) {
    let (commands, command_receiver) = mpsc::channel();
    let (notifications, notification_receiver) = mpsc::channel();
    let frames = Arc::new(FrameHub::new(None));
    let presenter = PresenterLink {
        commands,
        notifications: notification_receiver,
        frames: Arc::clone(&frames),
        requester: Arc::clone(&requester),
        frame: None,
        events: Vec::new(),
        listener_names: FxHashSet::default(),
        begin_frames_sent: 0,
        begin_frames_serviced: 0,
        redraw_pending: Cell::new(false),
    };
    let main = MainLink {
        commands: command_receiver,
        notify: ToPresenterSender {
            notifications,
            frames,
            requester,
        },
    };
    (presenter, main)
}

impl<R: EventRequester> PresenterLink<R> {
    /// Sends one command. A closed channel is a main thread that has exited;
    /// the presenting side goes on showing what it last published.
    pub(crate) fn send(&self, command: ToMain) {
        let _ = self.commands.send(command);
    }

    /// Applies everything that has arrived. However many frames were
    /// announced, the mailbox is read once.
    pub(crate) fn sync(&mut self) {
        let mut announced = false;
        while let Ok(notification) = self.notifications.try_recv() {
            announced |= self.apply(notification);
        }
        self.adopt_frame(announced);
    }

    /// Applies one notification, answering whether it announced a frame.
    fn apply(&mut self, notification: ToPresenter) -> bool {
        match notification {
            ToPresenter::FrameChanged => return true,
            ToPresenter::Engine(event) => self.events.push(event),
            ToPresenter::ListenerAvailable(name) => {
                self.listener_names.insert(name);
            }
            ToPresenter::ListenerUnavailable(name) => {
                self.listener_names.remove(&name);
            }
            ToPresenter::BeginFrameServiced(seq) => {
                self.begin_frames_serviced = self.begin_frames_serviced.max(seq);
            }
        }
        false
    }

    /// Reads the mailbox, once, when a batch announced anything at all.
    fn adopt_frame(&mut self, announced: bool) {
        if announced {
            self.frame.clone_from(&slot(&self.frames));
            self.redraw_pending.set(true);
        }
    }

    /// The newest published frame as of the last sync.
    pub(crate) fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.frame.as_ref()
    }

    /// Whether the realm had a listener for `name` as of the last sync.
    pub(crate) fn has_listener(&self, name: &str) -> bool {
        self.listener_names.contains(name)
    }

    /// Hands the embedder every lifecycle event drained so far.
    pub(crate) fn take_events(&mut self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.events)
    }

    /// Asks for a frame on the presenting side's own behalf, and wakes the
    /// host loop that will draw it.
    pub(crate) fn request_redraw(&self) {
        self.redraw_pending.set(true);
        self.requester.request_event();
    }

    /// Whether a frame is owed, clearing the request.
    pub(crate) fn take_redraw(&self) -> bool {
        self.redraw_pending.replace(false)
    }

    /// Asks the main thread to advance the timeline to `now`, answering with
    /// the sequence number [`Self::wait_begin_frame`] waits on.
    pub(crate) fn begin_frame(&mut self, now: f64) -> Option<u64> {
        self.begin_frames_sent += 1;
        let seq = self.begin_frames_sent;
        self.commands
            .send(ToMain::BeginFrame { now, seq })
            .ok()
            .map(|()| seq)
    }

    /// Blocks until `seq` has been serviced, the main thread exits, or the
    /// deadline passes — applying, not dropping, everything that arrives on
    /// the way. The commit a `BeginFrame` produces is announced before the
    /// acknowledgement is, so a wait that answers `true` has the frame.
    pub(crate) fn wait_begin_frame(&mut self, seq: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut announced = false;
        while self.begin_frames_serviced < seq {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let Ok(notification) = self.notifications.recv_timeout(remaining) else {
                break;
            };
            announced |= self.apply(notification);
        }
        self.adopt_frame(announced);
        self.begin_frames_serviced >= seq
    }

    /// Every notification that has arrived, raw and in order — for tests
    /// that assert what crossed rather than what it added up to.
    #[cfg(test)]
    pub(crate) fn drain(&mut self) -> Vec<ToPresenter> {
        self.notifications.try_iter().collect()
    }
}

impl<R: EventRequester> fmt::Debug for PresenterLink<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PresenterLink")
            .field("listener_names", &self.listener_names.len())
            .field("begin_frames_sent", &self.begin_frames_sent)
            .finish_non_exhaustive()
    }
}
