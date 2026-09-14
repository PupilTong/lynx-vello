//! A realm asks its painter for the next vsync and receives one timestamp.
//! The painter owns the request queue. JS owns every callback and callback ID.

use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;

use quickjs_rust_bridge::HostValue;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;
use crate::view::EventRequester;

/// The realm's sending end. Replies go directly back to the requesting realm.
#[derive(Clone)]
pub(crate) struct VsyncRequester {
    requests: mpsc::UnboundedSender<oneshot::Sender<f64>>,
    wakeup: Arc<dyn EventRequester>,
}

/// The painter's receiving end, retained by the view across painter attachment.
pub(crate) struct VsyncRequests {
    incoming: mpsc::UnboundedReceiver<oneshot::Sender<f64>>,
    pending: Vec<oneshot::Sender<f64>>,
}

impl VsyncRequests {
    pub(crate) fn new(wakeup: Arc<dyn EventRequester>) -> (VsyncRequester, Self) {
        let (requests, incoming) = mpsc::unbounded_channel();
        (
            VsyncRequester { requests, wakeup },
            Self {
                incoming,
                pending: Vec::new(),
            },
        )
    }

    /// Called by the painter when the host asks whether to request vsync.
    pub(crate) fn is_pending(&mut self) -> bool {
        while let Ok(request) = self.incoming.try_recv() {
            self.pending.push(request);
        }
        self.pending.retain(|request| !request.is_closed());
        !self.pending.is_empty()
    }

    /// Answer the requests present at this vsync. Requests made by callbacks
    /// remain in the inbox until the host delivers another vsync.
    pub(crate) fn dispatch(&mut self, milliseconds: f64) -> bool {
        self.is_pending();
        let mut sent = false;
        for request in self.pending.drain(..) {
            sent |= request.send(milliseconds).is_ok();
        }
        sent
    }
}

/// One realm's outstanding request. Dropping its reply cancels it at the painter.
pub(crate) struct AnimationFrames {
    requester: VsyncRequester,
    reply: RefCell<Option<oneshot::Receiver<f64>>>,
    changed: Notify,
}

impl AnimationFrames {
    pub(crate) fn new(requester: VsyncRequester) -> Rc<Self> {
        Rc::new(Self {
            requester,
            reply: RefCell::new(None),
            changed: Notify::new(),
        })
    }

    pub(crate) fn set(&self, pending: bool) {
        let mut reply = self.reply.borrow_mut();
        if pending == reply.is_some() {
            return;
        }
        *reply = if pending {
            let (request, answer) = oneshot::channel();
            let _ = self.requester.requests.send(request);
            Some(answer)
        } else {
            None
        };
        self.changed.notify_one();
        self.requester.wakeup.request_event();
    }

    /// MTS consumes its reply while handling the painter's Vsync command.
    pub(crate) fn take(&self) -> Option<f64> {
        let mut reply = self.reply.borrow_mut();
        match reply.as_mut()?.try_recv() {
            Ok(milliseconds) => {
                reply.take();
                Some(milliseconds)
            }
            Err(oneshot::error::TryRecvError::Closed) => {
                reply.take();
                None
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
        }
    }

    /// A worker waits on its own reply, just as its timer task waits on its
    /// own deadline. A cancellation or a new request wakes this waiter too.
    pub(crate) async fn next(&self) -> f64 {
        loop {
            tokio::select! {
                () = self.changed.notified() => {},
                result = poll_fn(|cx| {
                    self.reply.borrow_mut().as_mut().map_or(Poll::Pending, |reply| std::pin::Pin::new(reply).poll(cx))
                }) => {
                    self.reply.borrow_mut().take();
                    if let Ok(milliseconds) = result { return milliseconds; }
                }
            }
        }
    }
}

impl Drop for AnimationFrames {
    fn drop(&mut self) {
        if self.reply.get_mut().take().is_some() {
            self.requester.wakeup.request_event();
        }
    }
}

pub(crate) fn install(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    requests: &Rc<AnimationFrames>,
) -> Result<(), ScriptError> {
    let requests = Rc::clone(requests);
    engine.register_host_module_function(
        js,
        crate::esm::HOST_MODULE_SPECIFIER,
        "requestScriptFrame",
        1,
        Box::new(move |arguments| {
            let Some(HostValue::Boolean(pending)) = arguments.first() else {
                return Err("requestScriptFrame expects a boolean".to_owned());
            };
            requests.set(*pending);
            Ok(HostValue::Undefined)
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_vsync_answers_requests_and_nested_requests_wait_for_another() {
        let (sender, mut painter) = VsyncRequests::new(Arc::new(crate::NoWakeup));
        let main = AnimationFrames::new(sender.clone());
        let background = AnimationFrames::new(sender);
        main.set(true);
        background.set(true);
        assert!(painter.is_pending());
        assert_eq!(main.take(), None, "requesting vsync does not deliver one");
        painter.dispatch(10.0);
        assert_eq!(main.take(), Some(10.0));
        main.set(true);
        painter.dispatch(20.0);
        assert_eq!(main.take(), Some(20.0));
        assert_eq!(
            background.take(),
            Some(10.0),
            "each request gets its own vsync message"
        );
        assert!(!painter.is_pending());
        background.set(true);
        assert_eq!(background.take(), None);
        painter.dispatch(30.0);
        assert_eq!(background.take(), Some(30.0));
    }

    #[test]
    fn cancellation_and_realm_release_withdraw_only_their_own_request() {
        let (sender, mut painter) = VsyncRequests::new(Arc::new(crate::NoWakeup));
        let main = AnimationFrames::new(sender.clone());
        let background = AnimationFrames::new(sender);
        main.set(true);
        main.set(true);
        background.set(true);
        main.set(false);
        assert!(painter.is_pending());
        drop(background);
        assert!(!painter.is_pending());
        painter.dispatch(1.0);
        assert_eq!(main.take(), None);
    }
}
