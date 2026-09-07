//! The same addressed FIFO in both directions: main receives `ToMain`, the
//! host receives `ToPainter`. Only the host needs to defer sibling views.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

use flume::RecvTimeoutError;
use rustc_hash::FxHashMap;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use crate::view::ViewId;

/// `None` addresses the group; `Some` addresses one of its views.
type Message<M> = (Option<ViewId>, M);
pub(crate) type Sender<M> = flume::Sender<Message<M>>;

pub(crate) struct Mailbox<M> {
    receiver: flume::Receiver<Message<M>>,
    pending: RefCell<FxHashMap<ViewId, Vec<M>>>,
}

impl<M> Mailbox<M> {
    pub(crate) fn channel() -> (Sender<M>, Self) {
        let (sender, receiver) = flume::unbounded();
        (
            sender,
            Self {
                receiver,
                pending: RefCell::new(FxHashMap::default()),
            },
        )
    }

    pub(crate) fn register(&self, view: ViewId) {
        self.pending.borrow_mut().insert(view, Vec::new());
    }

    pub(crate) fn remove(&self, view: ViewId) {
        self.pending.borrow_mut().remove(&view);
    }

    fn defer(&self, (view, message): Message<M>) {
        let mut pending = self.pending.borrow_mut();
        if let Some(messages) = view.and_then(|view| pending.get_mut(&view)) {
            messages.push(message);
        }
    }

    pub(crate) fn drain(&self) -> impl Iterator<Item = Message<M>> + '_ {
        self.receiver.drain()
    }

    /// The host serves one view at a time. Preserve siblings' order without
    /// building a channel per view or buffering the active view twice.
    pub(crate) fn drain_view(&self, view: ViewId, mut apply: impl FnMut(M)) {
        let mut pending = self.pending.borrow_mut();
        for message in pending
            .get_mut(&view)
            .expect("a live link registers its view")
            .drain(..)
        {
            apply(message);
        }
        for (target, message) in self.drain() {
            if target == Some(view) {
                apply(message);
            } else if let Some(messages) = target.and_then(|target| pending.get_mut(&target)) {
                messages.push(message);
            }
        }
    }

    /// Both the main loop and offscreen frame waits use this deadline path.
    /// Flume's timed receive uses a native clock, so poll its future with a
    /// thread waker to support main-thread timers on Wasm as well.
    pub(crate) fn recv(&self, deadline: Option<Instant>) -> Result<Message<M>, RecvTimeoutError> {
        let Some(deadline) = deadline else {
            return self
                .receiver
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected);
        };
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut receiving = self.receiver.recv_async();
        loop {
            match Pin::new(&mut receiving).poll(&mut context) {
                Poll::Ready(result) => return result.map_err(|_| RecvTimeoutError::Disconnected),
                Poll::Pending => {}
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(RecvTimeoutError::Timeout)?;
            thread::park_timeout(remaining);
        }
    }

    pub(crate) fn wait_view(&self, deadline: Instant) -> bool {
        let Ok(message) = self.recv(Some(deadline)) else {
            return false;
        };
        self.defer(message);
        true
    }

    pub(crate) fn is_disconnected(&self) -> bool {
        self.receiver.is_disconnected()
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&self) -> Result<Message<M>, flume::TryRecvError> {
        self.receiver.try_recv()
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
