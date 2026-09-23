//! URL-only style bindings. The fetcher owns preloads and cached source state.

use std::cell::RefCell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;

use super::{DocumentSlot, MainThreadError, ScriptEngine, ScriptRuntime, install};
use crate::jobs::JsThreadHandle;
use crate::link::{SourceAnswer, ViewOutbox};
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
        // A synchronous wait, the same shape a `require` makes (see
        // `crate::require`). It is
        // inside a job, so what it drives is this engine thread's tasks —
        // channel reads, lifecycle signals, acknowledgements, the routing that
        // answers this very request — and none of its jobs: no JavaScript of
        // this realm's or any sibling's runs before this returns. The view's
        // own token is first, so a release ends the wait rather than the
        // response doing it.
        let source = wait_for_source(&thread, &token, answer)
            .map_err(|error| format!("loading stylesheet {url}: {error}"))?;
        let mut slot = document.borrow_mut();
        mount_style_sheet(slot.document_mut(), url, source)?;
        Ok(HostValue::Undefined)
    })
}

/// Mounts one loaded author sheet, whichever form the fetcher answered in —
/// what `adoptStyleSheet` does with its answer.
pub(super) fn mount_style_sheet(
    document: &mut crate::main::tree::LynxDocument,
    url: &str,
    source: LoadedSource,
) -> Result<(), String> {
    match source {
        LoadedSource::StyleSheet(sheet) => {
            add_style_sheet(document, sheet);
            Ok(())
        }
        LoadedSource::Entry { .. } => Err(format!("stylesheet {url} returned a script")),
        LoadedSource::Font(_) => Err(format!("stylesheet {url} returned a font")),
        LoadedSource::Fetched => Err(format!("stylesheet {url} returned a plain fetch")),
    }
}

/// Appends one author sheet to the document's cascade, after every sheet it
/// already holds. Shared with the view's startup sheets, which a task of the
/// view's owner mounts as each answer arrives.
pub(super) fn add_style_sheet(
    document: &mut crate::main::tree::LynxDocument,
    sheet: StyleSheetSource,
) {
    match sheet {
        StyleSheetSource::Text(css) => crate::style::add_style_sheet_text(document, &css),
        StyleSheetSource::Preparsed(sheet) => {
            crate::style::add_preparsed_style_sheet(document, &sheet);
        }
    }
}

/// Waits inside a job for the answer to one `adoptStyleSheet` request, with the
/// realm's end as the first arm.
///
/// It parks the *job* it runs in: every task of the engine thread goes on
/// running, including the one routing this very answer, and no other job
/// does, so the realm holds its borrows across it. The token is biased first,
/// so a release ends the wait rather than the answer doing it. An answer
/// already in hand costs no wait at all.
fn wait_for_source(
    thread: &JsThreadHandle,
    token: &tokio_util::sync::CancellationToken,
    mut answer: SourceAnswer,
) -> Result<LoadedSource, String> {
    use tokio::sync::oneshot::error::TryRecvError;
    let reason = |answered: Result<LoadedSource, crate::view::LynxViewError>| {
        answered.map_err(|error| error.to_string())
    };
    match answer.try_recv() {
        Ok(answered) => reason(answered),
        Err(TryRecvError::Closed) => reason(Err(unanswered_source().into())),
        Err(TryRecvError::Empty) => thread.wait(async {
            tokio::select! {
                biased;
                () = token.cancelled() => Err("view was released".to_owned()),
                result = answer => reason(
                    result.unwrap_or_else(|_| Err(unanswered_source().into())),
                ),
            }
        }),
    }
}
