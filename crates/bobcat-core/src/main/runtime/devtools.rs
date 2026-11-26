//! The page session on `bobcat-main`: its state, the routing of its
//! messages to the domains, and the hooks the runtime calls.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use terminus::link::NodeId;

use super::{DocumentSlot, MainThreadRuntime};
use crate::devtools::protocol::{self, Error, Request};
use crate::devtools::{Attachment, Frontend, Outlet, css, dom, overlay, runtime, target};
use crate::link::InputEventPayload;
use crate::main::quickjs::ScriptRuntime;
use crate::main::tree::LynxDocument;
use crate::script::ScriptError;

pub(crate) struct PageSession {
    frontend: Frontend,
    runtime: RefCell<runtime::RuntimeState>,
    dom: RefCell<dom::DomState>,
    dom_pending: Cell<bool>,
    css: RefCell<css::CssState>,
    overlay: RefCell<overlay::OverlayState>,
    attachment: Attachment,
}

impl PageSession {
    fn document_changed(&self, document: &mut LynxDocument) {
        match self.dom.try_borrow_mut() {
            Ok(mut dom) => dom::document_changed(&mut dom, document, &self.frontend),
            Err(_) => self.dom_pending.set(true),
        }
    }
}

pub(crate) struct DevtoolsDocument<'a>(&'a mut DocumentSlot);

impl DevtoolsDocument<'_> {
    pub(crate) fn document(&mut self) -> Option<&mut LynxDocument> {
        self.0.document.as_mut()
    }

    pub(crate) fn route_names(
        &mut self,
        names: &[&str],
        change: impl FnOnce(&mut overlay::Routes),
    ) {
        self.0.apply_pending_last_removals();
        self.0
            .change_names(names, |slot| change(&mut slot.devtools_routes));
    }
}

impl DocumentSlot {
    #[inline]
    pub(super) fn devtools_document_changed(&mut self) {
        if let Some(session) = self.devtools.as_ref().and_then(Weak::upgrade)
            && let Some(document) = self.document.as_mut()
        {
            session.document_changed(document);
        }
    }
}

impl MainThreadRuntime {
    pub(crate) fn devtools_attach(&mut self, attachment: Attachment) -> Arc<Outlet> {
        self.devtools_detach();
        let outlet = Arc::clone(&attachment.outlet);
        let session = Rc::new(PageSession {
            frontend: Frontend::new(Arc::clone(&outlet), None),
            runtime: RefCell::default(),
            dom: RefCell::default(),
            dom_pending: Cell::new(false),
            css: RefCell::default(),
            overlay: RefCell::default(),
            attachment,
        });
        self.slot.borrow_mut().devtools = Some(Rc::downgrade(&session));
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
        let Some(session) = self.devtools.take() else {
            return;
        };
        {
            let mut slot = self.slot.borrow_mut();
            slot.devtools = None;
            overlay::detach(
                &mut session.overlay.borrow_mut(),
                &mut DevtoolsDocument(&mut slot),
            );
        }
        runtime::detach(&mut session.runtime.borrow_mut(), &self.core);
        target::detach(self.workers.devtools_auto_attach());
    }

    pub(crate) fn devtools_message(
        &self,
        js: &mut ScriptRuntime,
        outlet: &Arc<Outlet>,
        text: &str,
    ) -> Result<(), ScriptError> {
        let Some(session) = self
            .devtools
            .clone()
            .filter(|session| session.frontend.is(outlet))
        else {
            return Ok(());
        };
        self.slot.borrow_mut().devtools_document_changed();
        let frontend = &session.frontend;
        let envelope = match protocol::parse(text) {
            Ok(envelope) => envelope,
            Err((id, error)) => {
                frontend.error(id, &error);
                return Ok(());
            }
        };
        let request = &envelope.request;
        if envelope.session_id.is_some() {
            frontend.error(Some(request.id), &Error::session_not_found());
            return Ok(());
        }
        let mut left = Ok(());
        match request.domain() {
            "Target" => target::dispatch(
                frontend,
                &session.attachment.routes,
                self.workers.devtools_auto_attach(),
                || self.workers.devtools_live_workers(),
                request,
            ),
            "Runtime" | "Console" => {
                left = runtime::dispatch(
                    &mut session.runtime.borrow_mut(),
                    &self.core,
                    js,
                    frontend,
                    request,
                    true,
                );
            }
            "DOM" => self.devtools_document(frontend, request, |document| {
                dom::dispatch(&mut session.dom.borrow_mut(), document, frontend, request);
            }),
            "CSS" => self.devtools_document(frontend, request, |document| {
                css::dispatch(&mut session.css.borrow_mut(), document, frontend, request);
            }),
            "Overlay" => overlay::dispatch(
                &mut session.overlay.borrow_mut(),
                &mut DevtoolsDocument(&mut self.slot.borrow_mut()),
                frontend,
                request,
            ),
            _ => frontend.not_found(request),
        }
        if session.dom_pending.take() {
            self.slot.borrow_mut().devtools_document_changed();
        }
        left
    }

    fn devtools_document(
        &self,
        frontend: &Frontend,
        request: &Request,
        dispatch: impl FnOnce(&mut LynxDocument),
    ) {
        match self.slot.borrow_mut().document.as_mut() {
            Some(document) => dispatch(document),
            None => frontend.error(
                Some(request.id),
                &Error::server("the page has no document yet"),
            ),
        }
    }

    #[inline]
    pub(super) fn devtools_takes_input(
        &self,
        target: NodeId,
        name: &str,
        payload: &InputEventPayload,
    ) -> bool {
        let Some(session) = &self.devtools else {
            return false;
        };
        let (Ok(mut state), Ok(mut slot)) =
            (session.overlay.try_borrow_mut(), self.slot.try_borrow_mut())
        else {
            return false;
        };
        overlay::input_event(
            &mut state,
            &mut DevtoolsDocument(&mut slot),
            &session.frontend,
            target,
            name,
            payload,
        )
    }
}
