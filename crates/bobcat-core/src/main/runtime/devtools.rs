//! The page session on `bobcat-main`: its state and the routing of its
//! messages to the domains.

use std::rc::Rc;
use std::sync::Arc;

use super::MainThreadRuntime;
use crate::devtools::protocol::{self, Error};
use crate::devtools::{Attachment, Frontend, Outlet, target};

pub(crate) struct PageSession {
    frontend: Frontend,
    attachment: Attachment,
}

impl MainThreadRuntime {
    pub(crate) fn devtools_attach(&mut self, attachment: Attachment) -> Arc<Outlet> {
        self.devtools_detach();
        let outlet = Arc::clone(&attachment.outlet);
        let session = Rc::new(PageSession {
            frontend: Frontend::new(Arc::clone(&outlet), None),
            attachment,
        });
        self.devtools = Some(session);
        outlet
    }

    pub(crate) fn devtools_end(&mut self, outlet: &Arc<Outlet>) {
        if self
            .devtools
            .as_ref()
            .is_some_and(|session| session.frontend.is(outlet))
        {
            self.devtools_detach();
        }
    }

    fn devtools_detach(&mut self) {
        let Some(_session) = self.devtools.take() else {
            return;
        };
        target::detach(self.workers.devtools_auto_attach());
    }

    pub(crate) fn devtools_message(&self, outlet: &Arc<Outlet>, text: &str) {
        let Some(session) = self
            .devtools
            .clone()
            .filter(|session| session.frontend.is(outlet))
        else {
            return;
        };
        let frontend = &session.frontend;
        let envelope = match protocol::parse(text) {
            Ok(envelope) => envelope,
            Err((id, error)) => {
                frontend.error(id, &error);
                return;
            }
        };
        let request = &envelope.request;
        if envelope.session_id.is_some() {
            frontend.error(Some(request.id), &Error::session_not_found());
            return;
        }
        match request.domain() {
            "Target" => target::dispatch(
                frontend,
                &session.attachment.routes,
                self.workers.devtools_auto_attach(),
                || self.workers.devtools_live_workers(),
                request,
            ),
            _ => frontend.not_found(request),
        }
    }
}
