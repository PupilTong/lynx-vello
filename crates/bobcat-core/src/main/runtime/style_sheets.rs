//! URL-only style bindings. The fetcher owns preloads and cached source state.

use std::cell::RefCell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use super::{DocumentSlot, MainThreadError, ScriptEngine, ScriptRuntime, install};
use crate::jobs::JsThreadHandle;
use crate::link::ViewOutbox;
use crate::resource::{LoadedSource, SourceRequest, StyleSheetSource, unanswered_source};

pub(super) fn install_styles(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    document: &Rc<RefCell<DocumentSlot>>,
    outbox: &ViewOutbox,
    thread: JsThreadHandle,
) -> Result<(), MainThreadError> {
    let sources = outbox.host_outbox(outbox.token().clone());
    install(engine, js, "preloadStyleSheet", 1, move |args| {
        let Some(HostValue::String(url)) = args.first() else {
            return Err("preloadStyleSheet expects a URL".to_owned());
        };
        sources.preload(SourceRequest::StyleSheet(url.clone()));
        Ok(HostValue::Undefined)
    })?;

    let document = Rc::clone(document);
    let token = outbox.token().clone();
    let sources = outbox.host_outbox(token.clone());
    install(engine, js, "adoptStyleSheet", 1, move |args| {
        let Some(HostValue::String(url)) = args.first() else {
            return Err("adoptStyleSheet expects a URL".to_owned());
        };
        // This receiver lives only for this call. Cache lookup and sharing an
        // in-flight preload belong to the resource fetcher on the host thread.
        let answer = sources.request(SourceRequest::StyleSheet(url.clone()));
        // The same synchronous wait a `require` makes (see `crate::require`).
        // It is inside a job, so what it drives is this engine thread's tasks
        // — channel reads, lifecycle signals, acknowledgements, the routing
        // that answers this very request — and none of its jobs: no JavaScript
        // of this realm's or any sibling's runs before this returns. The
        // view's own token is first, so a release ends the wait rather than
        // the response doing it.
        let source = thread
            .wait(async {
                tokio::select! {
                    biased;
                    () = token.cancelled() => Err("view was released".to_owned()),
                    result = answer => result
                        .unwrap_or_else(|_| Err(unanswered_source().into()))
                        .map_err(|error| error.to_string()),
                }
            })
            .map_err(|error| format!("loading stylesheet {url}: {error}"))?;
        let mut slot = document.borrow_mut();
        match source {
            LoadedSource::StyleSheet(StyleSheetSource::Text(css)) => {
                crate::style::add_style_sheet_text(slot.document_mut(), &css);
            }
            LoadedSource::StyleSheet(StyleSheetSource::Preparsed(sheet)) => {
                crate::style::add_preparsed_style_sheet(slot.document_mut(), &sheet);
            }
            LoadedSource::Entry { .. } => {
                return Err(format!("stylesheet {url} returned a script"));
            }
            LoadedSource::Font(_) => {
                return Err(format!("stylesheet {url} returned a font"));
            }
        }
        Ok(HostValue::Undefined)
    })
}
