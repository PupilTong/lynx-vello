//! The `Target` domain of the page session: worker auto-attach.

use std::cell::RefCell;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::protocol::{Error, Request};
use super::worker::{WorkerDevtools, WorkerOp};
use super::{Frontend, WorkerRoutes, worker_session_id};
use crate::background::{WorkerKey, WorkerMessage};

pub(crate) type LiveWorker = (WorkerKey, Arc<str>, mpsc::UnboundedSender<WorkerMessage>);

pub(crate) struct AutoAttach {
    frontend: Frontend,
    routes: Arc<WorkerRoutes>,
}

impl AutoAttach {
    pub(crate) fn worker_started(
        &self,
        key: WorkerKey,
        url: &str,
        messages: &mpsc::UnboundedSender<WorkerMessage>,
    ) {
        let session_id = worker_session_id(key);
        self.routes
            .insert(Arc::clone(&session_id), messages.downgrade());
        let _ = messages.send(WorkerMessage::Devtools(Box::new(WorkerDevtools {
            outlet: Arc::clone(self.frontend.outlet()),
            op: WorkerOp::Attach {
                session_id: Arc::clone(&session_id),
            },
        })));
        self.frontend.event(
            "Target.attachedToTarget",
            &AttachedToTarget {
                session_id: &session_id,
                target_info: TargetInfo {
                    target_id: &session_id,
                    kind: "worker",
                    title: url,
                    url,
                    attached: true,
                    can_access_opener: false,
                },
                waiting_for_debugger: false,
            },
        );
    }

    pub(crate) fn worker_ended(&self, key: WorkerKey) {
        let session_id = worker_session_id(key);
        if self.routes.remove(&session_id) {
            self.announce_detached(&session_id);
        }
    }

    fn detach_all(&self, announce: bool) {
        for (session_id, messages) in self.routes.take() {
            if let Some(messages) = messages.upgrade() {
                let _ = messages.send(WorkerMessage::Devtools(Box::new(WorkerDevtools {
                    outlet: Arc::clone(self.frontend.outlet()),
                    op: WorkerOp::Detach,
                })));
            }
            if announce {
                self.announce_detached(&session_id);
            }
        }
    }

    fn announce_detached(&self, session_id: &str) {
        self.frontend.event(
            "Target.detachedFromTarget",
            &DetachedFromTarget {
                session_id,
                target_id: session_id,
            },
        );
    }
}

pub(crate) fn dispatch(
    frontend: &Frontend,
    routes: &Arc<WorkerRoutes>,
    auto_attach: &RefCell<Option<AutoAttach>>,
    live: impl FnOnce() -> Vec<LiveWorker>,
    request: &Request,
) {
    match request.method.as_str() {
        "Target.setAutoAttach" => frontend.reply(
            request.id,
            request
                .params::<SetAutoAttach>()
                .and_then(|params| set_auto_attach(frontend, routes, auto_attach, live, &params)),
        ),
        _ => frontend.not_found(request),
    }
}

pub(crate) fn detach(auto_attach: &RefCell<Option<AutoAttach>>) {
    let attached = auto_attach.borrow_mut().take();
    if let Some(attached) = attached {
        attached.detach_all(false);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAutoAttach {
    auto_attach: bool,
    #[serde(rename = "waitForDebuggerOnStart")]
    _wait_for_debugger_on_start: bool,
    #[serde(default)]
    flatten: bool,
}

#[derive(Serialize)]
struct Empty {}

fn set_auto_attach(
    frontend: &Frontend,
    routes: &Arc<WorkerRoutes>,
    auto_attach: &RefCell<Option<AutoAttach>>,
    live: impl FnOnce() -> Vec<LiveWorker>,
    params: &SetAutoAttach,
) -> Result<Empty, Error> {
    if !params.flatten {
        return Err(Error::invalid_params(
            "only flat sessions exist: `flatten` must be true",
        ));
    }
    if !params.auto_attach {
        let attached = auto_attach.borrow_mut().take();
        if let Some(attached) = attached {
            attached.detach_all(true);
        }
        return Ok(Empty {});
    }
    if auto_attach.borrow().is_some() {
        return Ok(Empty {});
    }
    let attached = AutoAttach {
        frontend: frontend.clone(),
        routes: Arc::clone(routes),
    };
    for (key, url, messages) in live() {
        attached.worker_started(key, &url, &messages);
    }
    *auto_attach.borrow_mut() = Some(attached);
    Ok(Empty {})
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachedToTarget<'a> {
    session_id: &'a str,
    target_info: TargetInfo<'a>,
    waiting_for_debugger: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetInfo<'a> {
    target_id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    title: &'a str,
    url: &'a str,
    attached: bool,
    can_access_opener: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DetachedFromTarget<'a> {
    session_id: &'a str,
    target_id: &'a str,
}
