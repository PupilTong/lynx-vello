//! A worker realm's devtools session on `bobcat-workers`.

use std::sync::Arc;

use super::{Frontend, Outlet, protocol};

pub(crate) struct WorkerDevtools {
    pub(crate) outlet: Arc<Outlet>,
    pub(crate) op: WorkerOp,
}

pub(crate) enum WorkerOp {
    Attach { session_id: Arc<str> },
    Message(String),
    Detach,
}

pub(crate) struct WorkerSession {
    frontend: Frontend,
}

pub(crate) fn handle(state: &mut Option<WorkerSession>, message: WorkerDevtools) {
    let WorkerDevtools { outlet, op } = message;
    match op {
        WorkerOp::Attach { session_id } => {
            *state = Some(WorkerSession {
                frontend: Frontend::new(outlet, Some(session_id)),
            });
        }
        WorkerOp::Message(text) => {
            if let Some(session) = state
                .as_mut()
                .filter(|session| session.frontend.is(&outlet))
            {
                dispatch(session, &text);
            }
        }
        WorkerOp::Detach => {
            state.take_if(|session| session.frontend.is(&outlet));
        }
    }
}

fn dispatch(session: &mut WorkerSession, text: &str) {
    let frontend = &session.frontend;
    let request = match protocol::parse(text) {
        Ok(envelope) => envelope.request,
        Err((id, error)) => {
            frontend.error(id, &error);
            return;
        }
    };
    frontend.not_found(&request);
}
