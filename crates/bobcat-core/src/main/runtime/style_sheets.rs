//! URL-only style bindings. The fetcher owns preloads and cached source state.

use std::cell::RefCell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
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
    // Asked before `__LoadStyleSheet` hands out a handle: a stylesheet the
    // fetcher says it does not hold answers `null` there, which is what a
    // container without that section answers natively. Synchronous and
    // request-free — the fetcher's probe reads what is registered and starts
    // nothing. Without a probe every URL may be held, so the adoption finds
    // out, as it always did.
    let probe = outbox.style_sheet_probe().cloned();
    install(engine, "hasStyleSheet", 1, move |args| {
        let Some(HostValue::String(url)) = args.first() else {
            return Err("hasStyleSheet expects a URL".to_owned());
        };
        Ok(HostValue::Boolean(
            probe.as_ref().is_none_or(|probe| probe(url)),
        ))
    })?;

    let sources = outbox.host_outbox(outbox.token().clone());
    install(engine, "preloadStyleSheet", 1, move |args| {
        let Some(HostValue::String(url)) = args.first() else {
            return Err("preloadStyleSheet expects a URL".to_owned());
        };
        sources.preload(SourceRequest::StyleSheet(url.clone()));
        Ok(HostValue::Undefined)
    })?;

    let document = Rc::clone(document);
    let token = outbox.token().clone();
    let sources = outbox.host_outbox(token.clone());
    // `adoptStyleSheet(url, shadowRoot?)`: the second argument names a shadow
    // root to mount on — a frame's, where `bobcat:frame-boot` mounts its
    // card's page CSS, so it styles the nested tree alone — and without it
    // the sheet is the document's.
    install(engine, "adoptStyleSheet", 1, move |args| {
        const NAME: &str = "bobcat-internal:host.adoptStyleSheet";
        let Some(HostValue::String(url)) = args.first() else {
            return Err("adoptStyleSheet expects a URL".to_owned());
        };
        let target = optional_node_id_argument(NAME, args, 1)?;
        let mut slot = document.borrow_mut();
        // Before the request: a target that is no live shadow root asks for
        // nothing. Nothing frees it while this call waits, because no
        // JavaScript runs.
        if let Some(shadow) = target
            && !slot
                .document_mut()
                .get(shadow)
                .is_some_and(terminus::link::Node::is_shadow_root)
        {
            return Err(format!("{NAME}: {shadow:?} is not a live shadow root"));
        }
        // This receiver lives only for this call. Cache lookup and sharing an
        // in-flight preload belong to the resource fetcher on the host thread.
        let answer = sources.request(SourceRequest::StyleSheet(url.clone()));
        // A synchronous wait, the same shape a `require` makes (see
        // `crate::require`). It is inside a job, so what it drives is this
        // engine thread's tasks — channel reads, lifecycle signals,
        // acknowledgements, the routing that answers this very request — and
        // none of its jobs: no JavaScript of this realm's or any sibling's
        // runs before this returns. The view's own token is first, so a
        // release ends the wait rather than the response doing it.
        //
        // The listed sheets first: they are the page's own cascade and were
        // requested before this one, so an adoption a card makes while the
        // entry evaluates still lands behind them, as it did when they were
        // mounted at the document's construction. A frame's sheets are a
        // cascade of their own, in the frame's shadow tree, which the listed
        // sheets have no order against.
        if target.is_none() {
            slot.settle_sheets(&thread)?;
        }
        settle_style_sheet(slot.document_mut(), &thread, &token, url, answer, target)?;
        Ok(HostValue::Undefined)
    })
}

/// Waits inside a job for the answer to one author stylesheet request, then
/// mounts it on `shadow` — one shadow root, or the document for `None` —
/// after every sheet it already holds.
///
/// The one path both kinds of author sheet take: `adoptStyleSheet` with the
/// answer to the request it just made, and the first `__FlushElementTree`
/// with each sheet the view listed, in listed order, from the answers
/// `create_lynx_view` asked for. A load that failed, and an answer that is not
/// a stylesheet, are an error naming `url` and the reason, which the host
/// member that called this throws.
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
        Ok(LoadedSource::Fetched) => Err("the fetcher returned a plain fetch".to_owned()),
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

/// Waits inside a job for one answer the realm is already owed, with the
/// realm's end as the first arm.
///
/// It parks the *job* it runs in: every task of the engine thread goes on
/// running, including the one routing this very answer, and no other job
/// does, so the realm holds its borrows across it. The token is biased first,
/// so a release ends the wait rather than the answer doing it. An answer
/// already in hand — a listed sheet the fetcher answered before the first
/// flush — costs no wait at all.
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
