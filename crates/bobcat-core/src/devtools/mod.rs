//! The CDP backend of one view: the public session and sink, the outlet
//! every answer leaves through, and the worker route table.
//! `crates/bobcat-core/docs/devtools.md` is the description.

pub(crate) mod protocol;
pub(crate) mod runtime;
pub(crate) mod target;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
pub(crate) mod worker;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use rustc_hash::FxHashMap;
use tokio::sync::mpsc;

pub(crate) use self::protocol::Frontend;
use crate::background::{WorkerKey, WorkerMessage};

pub trait DevtoolsSink: Send + Sync + 'static {
    fn message(&self, message: String);
    fn closed(&self);
}

pub struct DevtoolsSession {
    outlet: Arc<Outlet>,
    routes: Arc<WorkerRoutes>,
    messages: mpsc::UnboundedSender<String>,
}

impl DevtoolsSession {
    pub(crate) fn new(sink: Arc<dyn DevtoolsSink>) -> (Self, Box<Attach>) {
        let outlet = Arc::new(Outlet {
            sink: Mutex::new(Some(sink)),
        });
        let routes = Arc::new(WorkerRoutes::default());
        let (messages, receiver) = mpsc::unbounded_channel();
        let attach = Box::new(Attach {
            messages: receiver,
            attachment: Attachment {
                outlet: Arc::clone(&outlet),
                routes: Arc::clone(&routes),
            },
        });
        (
            Self {
                outlet,
                routes,
                messages,
            },
            attach,
        )
    }

    pub(crate) fn outlet(&self) -> Weak<Outlet> {
        Arc::downgrade(&self.outlet)
    }

    pub fn send(&self, message: String) {
        if let Some(id) = protocol::session_id(&message)
            && let Some(worker) = self.routes.get(&id)
        {
            let _ = worker.send(WorkerMessage::Devtools(Box::new(worker::WorkerDevtools {
                outlet: Arc::clone(&self.outlet),
                op: worker::WorkerOp::Message(message),
            })));
            return;
        }
        let _ = self.messages.send(message);
    }
}

impl Drop for DevtoolsSession {
    fn drop(&mut self) {
        self.outlet.release();
    }
}

impl std::fmt::Debug for DevtoolsSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DevtoolsSession")
            .finish_non_exhaustive()
    }
}

pub(crate) struct Outlet {
    sink: Mutex<Option<Arc<dyn DevtoolsSink>>>,
}

impl Outlet {
    fn lock(&self) -> MutexGuard<'_, Option<Arc<dyn DevtoolsSink>>> {
        self.sink.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn send(&self, message: String) {
        if let Some(sink) = self.lock().as_ref() {
            sink.message(message);
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.lock().is_some()
    }

    fn release(&self) {
        self.lock().take();
    }

    fn close(&self) {
        let sink = self.lock().take();
        if let Some(sink) = sink {
            sink.closed();
        }
    }
}

#[derive(Default)]
pub(crate) struct WorkerRoutes(
    Mutex<FxHashMap<Arc<str>, mpsc::WeakUnboundedSender<WorkerMessage>>>,
);

impl WorkerRoutes {
    fn lock(
        &self,
    ) -> MutexGuard<'_, FxHashMap<Arc<str>, mpsc::WeakUnboundedSender<WorkerMessage>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, session_id: &str) -> Option<mpsc::UnboundedSender<WorkerMessage>> {
        self.lock().get(session_id)?.upgrade()
    }

    pub(crate) fn insert(
        &self,
        session_id: Arc<str>,
        messages: mpsc::WeakUnboundedSender<WorkerMessage>,
    ) {
        self.lock().insert(session_id, messages);
    }

    pub(crate) fn remove(&self, session_id: &str) -> bool {
        self.lock().remove(session_id).is_some()
    }

    pub(crate) fn take(&self) -> Vec<(Arc<str>, mpsc::WeakUnboundedSender<WorkerMessage>)> {
        self.lock().drain().collect()
    }
}

pub(crate) struct Attach {
    pub(crate) messages: mpsc::UnboundedReceiver<String>,
    pub(crate) attachment: Attachment,
}

pub(crate) struct Attachment {
    pub(crate) outlet: Arc<Outlet>,
    pub(crate) routes: Arc<WorkerRoutes>,
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.outlet.close();
    }
}

pub(crate) fn worker_session_id(key: WorkerKey) -> Arc<str> {
    Arc::from(format!("worker-{}", key.get()))
}
