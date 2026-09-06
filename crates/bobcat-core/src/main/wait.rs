//! How a thread of this engine parks.
//!
//! Both threads here wait the same way and for the same reasons: for a message
//! on one or more FIFOs, and for the earliest deadline a realm armed. `flume`'s
//! own timed receive reads the standard library's clock, which wasm32 does not
//! implement, so the wait is assembled out of the two pieces both targets do
//! have — the receivers' futures, and `park_timeout`, which is exactly what
//! `flume` blocks on itself.
//!
//! Nothing drives those futures but [`park_until`], and its waker only unparks
//! the thread that called it, so this is a blocking wait spelled with futures
//! rather than an executor.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use super::runtime::ClockInstant;

/// What ended one wait.
pub(crate) enum Woken<T> {
    /// A message arrived; more may be queued behind it.
    Command(T),
    /// The earliest armed timer came due with no message to serve.
    Deadline,
    /// Every sender is gone, and nothing more will be asked of this thread.
    Disconnected,
}

/// Blocks until `poll` yields something, or until `deadline` passes.
///
/// `poll` is the caller's whole wait: it may look at one receiver or several,
/// in whatever order the caller's priorities call for, and it is re-run after
/// every wake. `None` means the deadline came first.
///
/// The waker handed to `poll` unparks this thread and does nothing else, so a
/// caller may register it on as many futures as it likes.
pub(crate) fn park_until<T>(
    deadline: Option<ClockInstant>,
    mut poll: impl FnMut(&mut Context<'_>) -> Poll<T>,
) -> Option<T> {
    let waker = Waker::from(Arc::new(UnparkWaker(thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(value) = poll(&mut context) {
            return Some(value);
        }
        let Some(deadline) = deadline else {
            // A spurious wake just polls again; a real one has already queued
            // whatever the next poll will find.
            thread::park();
            continue;
        };
        let remaining = deadline.checked_duration_since(ClockInstant::now())?;
        thread::park_timeout(remaining);
    }
}

/// The wait for a thread with exactly one receiver: the worker thread, which
/// has no second channel and no group to serve.
pub(crate) fn wait_on<T>(
    commands: &flume::Receiver<T>,
    deadline: Option<ClockInstant>,
) -> Woken<T> {
    let Some(deadline) = deadline else {
        return commands.recv().map_or(Woken::Disconnected, Woken::Command);
    };
    let mut receiving = commands.recv_async();
    park_until(Some(deadline), |context| {
        match Pin::new(&mut receiving).poll(context) {
            Poll::Ready(Ok(command)) => Poll::Ready(Woken::Command(command)),
            Poll::Ready(Err(flume::RecvError::Disconnected)) => Poll::Ready(Woken::Disconnected),
            Poll::Pending => Poll::Pending,
        }
    })
    .unwrap_or(Woken::Deadline)
}

/// The waker [`park_until`] hands the caller: the only thing a send has to do
/// is end this thread's park.
struct UnparkWaker(thread::Thread);

impl Wake for UnparkWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
