//! URL-loaded stylesheet handles. Every load uses the view's resource fetcher.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;

use super::{DocumentSlot, MainThreadError, ScriptEngine, ScriptRuntime, install};
use crate::resource::{LoadedSource, StyleSheetSource};

#[derive(Default)]
pub(crate) struct Sheet {
    source: RefCell<Option<Result<StyleSheetSource, String>>>,
}

impl Sheet {
    pub(crate) fn complete(
        &self,
        result: Result<LoadedSource, crate::LynxViewError>,
    ) -> Result<(), String> {
        let result = match result {
            Ok(LoadedSource::StyleSheet(source)) => Ok(source),
            Ok(LoadedSource::Entry { .. }) => {
                Err("a stylesheet request returned a script".to_owned())
            }
            Err(error) => Err(error.to_string()),
        };
        let status = result.as_ref().map(|_| ()).map_err(Clone::clone);
        *self.source.borrow_mut() = Some(result);
        status
    }
}

/// Holds pending adoptions in call order, regardless of resource completion order.
/// A queued adoption retains its sheet even after JS releases the handle.
#[derive(Default)]
pub(crate) struct Styles {
    next: Cell<u64>,
    handles: RefCell<FxHashMap<String, Rc<Sheet>>>,
    requests: RefCell<VecDeque<(String, Rc<Sheet>)>>,
    adoptions: RefCell<VecDeque<Rc<Sheet>>>,
}

impl Styles {
    pub(crate) fn take_request(&self) -> Option<(String, Rc<Sheet>)> {
        self.requests.borrow_mut().pop_front()
    }

    pub(crate) fn apply(&self, slot: &mut DocumentSlot) {
        let mut adoptions = self.adoptions.borrow_mut();
        while let Some(sheet) = adoptions.front() {
            let source = sheet.source.borrow();
            match source.as_ref() {
                None => break,
                Some(Err(_)) => {}
                Some(Ok(StyleSheetSource::Text(css))) => {
                    crate::style::add_style_sheet_text(slot.document_mut(), css);
                }
                Some(Ok(StyleSheetSource::Preparsed(sheet))) => {
                    crate::style::add_preparsed_style_sheet(slot.document_mut(), sheet);
                }
            }
            drop(source);
            adoptions.pop_front();
        }
    }
}

pub(super) fn install_styles(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    document: &Rc<RefCell<DocumentSlot>>,
) -> Result<Rc<Styles>, MainThreadError> {
    let styles = Rc::new(Styles::default());
    for name in ["loadStyleSheet", "adoptStyleSheet", "releaseStyleSheet"] {
        let styles = Rc::clone(&styles);
        let document = Rc::clone(document);
        install(engine, js, name, 1, move |args| {
            let Some(HostValue::String(value)) = args.first() else {
                return Err(format!("{name} expects a string"));
            };
            match name {
                "loadStyleSheet" => {
                    let id = styles.next.get();
                    styles.next.set(
                        id.checked_add(1)
                            .expect("stylesheet handle space exhausted"),
                    );
                    let id = id.to_string();
                    let sheet = Rc::new(Sheet::default());
                    styles
                        .requests
                        .borrow_mut()
                        .push_back((value.clone(), Rc::clone(&sheet)));
                    styles.handles.borrow_mut().insert(id.clone(), sheet);
                    Ok(HostValue::String(id))
                }
                "adoptStyleSheet" => {
                    let sheet = styles
                        .handles
                        .borrow()
                        .get(value)
                        .cloned()
                        .ok_or_else(|| "stylesheet handle was released".to_owned())?;
                    styles.adoptions.borrow_mut().push_back(sheet);
                    styles.apply(&mut document.borrow_mut());
                    Ok(HostValue::Undefined)
                }
                "releaseStyleSheet" => {
                    styles.handles.borrow_mut().remove(value);
                    Ok(HostValue::Undefined)
                }
                _ => unreachable!(),
            }
        })?;
    }
    Ok(styles)
}
