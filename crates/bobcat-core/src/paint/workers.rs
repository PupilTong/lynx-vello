//! The painter's half of `Worker`: fetching worker scripts off the frame
//! path.
//!
//! A `Worker` is constructed inside a realm, on `bobcat-main`, which owns no
//! fetcher and never awaits anything. So the script is fetched here, on the
//! thread that does own one — the same split the view's startup sources use,
//! moved off construction and onto the painter's turns.
//!
//! # Why these are polled with the host's own wakeup
//!
//! The painter has no thread and no event loop: the host's turns *are* its
//! executor, and it runs only inside them. A fetch that becomes ready between
//! turns therefore has no way to cause the next poll — so the waker handed to
//! these futures is the group's `EventRequester` itself, which is what gives
//! the host a turn. Nothing waits on it; it is what makes an answer nobody is
//! waiting for observable at all.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use super::sources;
use crate::main::workers::WorkerKey;
use crate::resource::{RequestId, ResourceFetcher};
use crate::view::WorkerScript;

/// One fetch in flight, and the worker it answers.
struct PendingScript {
    key: WorkerKey,
    /// `'static` because it owns its handle on the resource system rather
    /// than borrowing the painter's: a future stored beside the store it
    /// reads would be a self-reference no borrow could express.
    load: Pin<Box<dyn Future<Output = Result<WorkerScript, String>>>>,
}

/// Every worker script this view is still waiting for.
#[derive(Default)]
pub(super) struct WorkerScripts {
    pending: Vec<PendingScript>,
    /// Namespaced per view the same way startup's are, so a host that keys
    /// its own bookkeeping on a request id sees no collision.
    requests: Option<RequestId>,
}

impl std::fmt::Debug for WorkerScripts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerScripts")
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl WorkerScripts {
    /// Starts one fetch. It is polled for the first time by the turn that
    /// started it, so a host that answers synchronously — a registry, a
    /// `data:` URL — never waits for a wakeup at all.
    pub(super) fn request<F: ResourceFetcher + 'static>(
        &mut self,
        fetcher: Rc<F>,
        key: WorkerKey,
        url: String,
    ) {
        let requests = self.requests.get_or_insert_with(sources::mint_namespace);
        let context = *requests;
        requests.sequence += 1;
        self.pending.push(PendingScript {
            key,
            load: Box::pin(async move {
                sources::load_worker_script(&*fetcher, context, &url)
                    .await
                    .map_err(|error| error.to_string())
            }),
        });
    }

    /// Polls every fetch in flight and answers with the ones that finished.
    ///
    /// Failures come back as text rather than as a `LynxViewError`: the only
    /// thing that will ever read one is the realm, as the `message` of an
    /// `error` event, and a view's construction is long over.
    pub(super) fn poll(&mut self, waker: &Waker) -> Vec<(WorkerKey, Result<WorkerScript, String>)> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let mut context = Context::from_waker(waker);
        let mut loaded = Vec::new();
        self.pending
            .retain_mut(|pending| match pending.load.as_mut().poll(&mut context) {
                Poll::Ready(script) => {
                    loaded.push((pending.key, script));
                    false
                }
                Poll::Pending => true,
            });
        loaded
    }
}
