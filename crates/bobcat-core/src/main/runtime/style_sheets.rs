//! Preload by URL, then synchronously obtain and adopt the sheet on demand.

use std::cell::RefCell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;
use tokio_util::sync::CancellationToken;

use super::{DocumentSlot, MainThreadError, ScriptEngine, ScriptRuntime, install};
use crate::link::{SourceAnswer, ViewOutbox, block_on};
use crate::resource::{LoadedSource, SourceRequest, StyleSheetSource, unanswered_source};

enum Sheet {
    Loading { url: String, answer: SourceAnswer },
    Ready(Result<StyleSheetSource, String>),
}

impl Sheet {
    fn source(&mut self, token: &CancellationToken) -> Result<&StyleSheetSource, String> {
        if let Self::Loading { url, answer } = self {
            // Only the embedder completes this request. Waiting neither enters
            // JavaScript again nor runs another task on the group's MTS thread.
            let source = block_on(async {
                tokio::select! {
                    biased;
                    () = token.cancelled() => Err("view was released".to_owned()),
                    result = answer => match result
                        .unwrap_or_else(|_| Err(unanswered_source().into()))
                    {
                        Ok(LoadedSource::StyleSheet(source)) => Ok(source),
                        Ok(LoadedSource::Entry { .. }) => Err("the fetcher returned a script".to_owned()),
                        Err(error) => Err(error.to_string()),
                    },
                }
            });
            *self =
                Self::Ready(source.map_err(|error| format!("loading stylesheet {url}: {error}")));
        }
        match self {
            Self::Ready(result) => result.as_ref().map_err(Clone::clone),
            Self::Loading { .. } => unreachable!(),
        }
    }
}

#[derive(Default)]
struct Handles {
    next: u64,
    sheets: FxHashMap<String, Sheet>,
}

pub(super) fn install_styles(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    document: &Rc<RefCell<DocumentSlot>>,
    outbox: &ViewOutbox,
) -> Result<(), MainThreadError> {
    let handles = Rc::new(RefCell::new(Handles::default()));
    for name in ["preloadStyleSheet", "adoptStyleSheet", "releaseStyleSheet"] {
        let handles = Rc::clone(&handles);
        let document = Rc::clone(document);
        let token = outbox.token().clone();
        let sources = outbox.source_requester(token.clone());
        install(engine, js, name, 1, move |args| {
            let Some(HostValue::String(value)) = args.first() else {
                return Err(format!("{name} expects a string"));
            };
            let mut handles = handles.borrow_mut();
            match name {
                "preloadStyleSheet" => {
                    let id = handles.next.to_string();
                    handles.next = handles
                        .next
                        .checked_add(1)
                        .expect("stylesheet handle space exhausted");
                    let answer = sources.request(SourceRequest::StyleSheet(value.clone()));
                    handles.sheets.insert(
                        id.clone(),
                        Sheet::Loading {
                            url: value.clone(),
                            answer,
                        },
                    );
                    Ok(HostValue::String(id))
                }
                "adoptStyleSheet" => {
                    let sheet = handles
                        .sheets
                        .get_mut(value)
                        .ok_or_else(|| "stylesheet handle was released".to_owned())?;
                    let source = sheet.source(&token)?;
                    let mut slot = document.borrow_mut();
                    match source {
                        StyleSheetSource::Text(css) => {
                            crate::style::add_style_sheet_text(slot.document_mut(), css);
                        }
                        StyleSheetSource::Preparsed(sheet) => {
                            crate::style::add_preparsed_style_sheet(slot.document_mut(), sheet);
                        }
                    }
                    Ok(HostValue::Undefined)
                }
                "releaseStyleSheet" => {
                    handles.sheets.remove(value);
                    Ok(HostValue::Undefined)
                }
                _ => unreachable!(),
            }
        })?;
    }
    Ok(())
}
