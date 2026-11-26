//! URL-only style bindings. The fetcher owns preloads and cached source state.

use std::cell::RefCell;
use std::rc::Rc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use tokio_util::sync::CancellationToken;

use super::{DocumentSlot, MainThreadError, ScriptEngine, install, optional_node_id_argument};
use crate::jobs::JsThreadHandle;
use crate::link::{SourceAnswer, ViewOutbox};
use crate::main::tree::LynxDocument;
use crate::resource::{LoadedSource, SourceRequest, StyleSheetSource, unanswered_source};

pub(super) fn install_styles(
    engine: &mut ScriptEngine,
    document: &Rc<RefCell<DocumentSlot>>,
    outbox: &ViewOutbox,
    thread: JsThreadHandle,
) -> Result<(), MainThreadError> {
    let probe = outbox.style_sheet_probe().cloned();
    install(engine, "hasStyleSheet", 1, move |args| {
        let Some(&HostArgument::String(url)) = args.first() else {
            return Err("hasStyleSheet expects a URL".to_owned());
        };
        Ok(HostValue::Boolean(
            probe.as_ref().is_none_or(|probe| probe(url)),
        ))
    })?;

    let sources = outbox.host_outbox(outbox.token().clone());
    install(engine, "preloadStyleSheet", 1, move |args| {
        let Some(&HostArgument::String(url)) = args.first() else {
            return Err("preloadStyleSheet expects a URL".to_owned());
        };
        sources.preload(SourceRequest::StyleSheet(url.to_owned()));
        Ok(HostValue::Undefined)
    })?;

    let document = Rc::clone(document);
    let token = outbox.token().clone();
    let sources = outbox.host_outbox(token.clone());
    install(engine, "adoptStyleSheet", 1, move |args| {
        const NAME: &str = "bobcat-internal:host.adoptStyleSheet";
        let Some(&HostArgument::String(url)) = args.first() else {
            return Err("adoptStyleSheet expects a URL".to_owned());
        };
        let target = optional_node_id_argument(NAME, args, 1)?;
        let mut slot = document.borrow_mut();
        if let Some(shadow) = target
            && !slot
                .document_mut()
                .get(shadow)
                .is_some_and(terminus::link::Node::is_shadow_root)
        {
            return Err(format!("{NAME}: {shadow:?} is not a live shadow root"));
        }
        let answer = sources.request(SourceRequest::StyleSheet(url.to_owned()));
        if target.is_none() {
            slot.settle_sheets(&thread)?;
        }
        settle_style_sheet(slot.document_mut(), &thread, &token, url, answer, target)?;
        Ok(HostValue::Undefined)
    })
}

pub(super) fn settle_style_sheet(
    document: &mut LynxDocument,
    thread: &JsThreadHandle,
    token: &CancellationToken,
    url: &str,
    answer: SourceAnswer,
    shadow: Option<terminus::link::NodeId>,
) -> Result<(), String> {
    let sheet = match wait_for_source(thread, token, answer) {
        Ok(LoadedSource::StyleSheet(sheet)) => Ok(sheet),
        Ok(LoadedSource::Module { .. }) => Err("the fetcher returned a script".to_owned()),
        Ok(LoadedSource::Font(_)) => Err("the fetcher returned a font".to_owned()),
        Ok(LoadedSource::Fetched(_)) => Err("the fetcher returned a plain fetch".to_owned()),
        Err(reason) => Err(reason),
    }
    .map_err(|reason| format!("loading stylesheet {url}: {reason}"))?;
    match (shadow, sheet) {
        (None, StyleSheetSource::Text(css)) => {
            crate::style::add_style_sheet_text(document, &css);
        }
        (None, StyleSheetSource::Preparsed(sheet)) => {
            crate::style::add_preparsed_style_sheet(document, &sheet);
        }
        (Some(shadow), StyleSheetSource::Text(css)) => {
            crate::style::add_style_sheet_text_to_shadow(document, shadow, &css);
        }
        (Some(shadow), StyleSheetSource::Preparsed(sheet)) => {
            crate::style::add_preparsed_style_sheet_to_shadow(document, shadow, &sheet);
        }
    }
    Ok(())
}

fn wait_for_source(
    thread: &JsThreadHandle,
    token: &CancellationToken,
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
