//! A worker realm's devtools session on `bobcat-workers`.

use std::sync::Arc;

use super::runtime::{self, RuntimeState};
use super::{Frontend, Outlet, protocol};
use crate::main::quickjs::ScriptRuntime;
use crate::realm::RealmCore;
use crate::script::ScriptError;

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
    runtime: RuntimeState,
}

impl WorkerSession {
    fn detach(mut self, core: &RealmCore) {
        runtime::detach(&mut self.runtime, core);
    }
}

pub(crate) fn handle(
    state: &mut Option<WorkerSession>,
    core: &RealmCore,
    js: &mut ScriptRuntime,
    message: WorkerDevtools,
    console_ready: bool,
) -> Result<(), ScriptError> {
    let WorkerDevtools { outlet, op } = message;
    match op {
        WorkerOp::Attach { session_id } => {
            if let Some(earlier) = state.take() {
                earlier.detach(core);
            }
            *state = Some(WorkerSession {
                frontend: Frontend::new(outlet, Some(session_id)),
                runtime: RuntimeState::default(),
            });
        }
        WorkerOp::Message(text) => {
            if let Some(session) = state
                .as_mut()
                .filter(|session| session.frontend.is(&outlet))
            {
                return dispatch(session, core, js, &text, console_ready);
            }
        }
        WorkerOp::Detach => {
            if let Some(session) = state.take_if(|session| session.frontend.is(&outlet)) {
                session.detach(core);
            }
        }
    }
    Ok(())
}

fn dispatch(
    session: &mut WorkerSession,
    core: &RealmCore,
    js: &mut ScriptRuntime,
    text: &str,
    console_ready: bool,
) -> Result<(), ScriptError> {
    let frontend = &session.frontend;
    let request = match protocol::parse(text) {
        Ok(envelope) => envelope.request,
        Err((id, error)) => {
            frontend.error(id, &error);
            return Ok(());
        }
    };
    match request.domain() {
        "Runtime" | "Console" => runtime::dispatch(
            &mut session.runtime,
            core,
            js,
            frontend,
            &request,
            console_ready,
        ),
        _ => {
            frontend.not_found(&request);
            Ok(())
        }
    }
}
