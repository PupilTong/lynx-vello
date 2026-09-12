//! MTS stylesheet handles over one document's already-decoded bundle styles.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;

use super::{DocumentSlot, MainThreadError, ScriptEngine, ScriptRuntime, install};
use crate::link::SourceRequester;

struct Styles {
    document: Rc<RefCell<DocumentSlot>>,
    sources: SourceRequester,
    next: Cell<u64>,
    handles: RefCell<FxHashMap<String, Vec<dom::CssRule>>>,
}

pub(super) fn install_styles(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    document: &Rc<RefCell<DocumentSlot>>,
    sources: SourceRequester,
) -> Result<(), MainThreadError> {
    let styles = Rc::new(Styles {
        document: Rc::clone(document),
        sources,
        next: Cell::new(0),
        handles: RefCell::default(),
    });
    for (name, arity) in [
        ("loadStyleSheet", 2),
        ("adoptStyleSheet", 1),
        ("releaseStyleSheet", 1),
        ("adoptComponentStyleSheet", 1),
    ] {
        let styles = Rc::clone(&styles);
        install(engine, js, name, arity, move |args| {
            let Some(HostValue::String(key)) = args.first() else {
                return Err(format!("{name} expects a string"));
            };
            match name {
                "adoptComponentStyleSheet" => {
                    let source = styles
                        .sources
                        .bundle(key)
                        .ok_or_else(|| "component bundle was not loaded".to_owned())?;
                    if let Some(sheet) = &source.style_sheet {
                        let mut document = styles.document.borrow_mut();
                        let document = document.document_mut();
                        let rules = crate::style::lower(document, sheet);
                        document.append_rules(rules);
                    }
                    Ok(HostValue::Undefined)
                }
                "loadStyleSheet" => {
                    let Some(HostValue::String(bundle)) = args.get(1) else {
                        return Err("loadStyleSheet expects a bundle name".to_owned());
                    };
                    let Some(source) = styles.sources.bundle(bundle) else {
                        return Ok(HostValue::Null);
                    };
                    let Some(sheet) = source.named_style_sheets.get(key) else {
                        return Ok(HostValue::Null);
                    };
                    // Parsing creates document-branded rules but changes no
                    // cascade. Re-adopting this handle reuses those same rules.
                    let rules =
                        crate::style::lower(styles.document.borrow_mut().document_mut(), sheet);
                    let id = styles.next.get();
                    styles.next.set(
                        id.checked_add(1)
                            .expect("stylesheet handle space exhausted"),
                    );
                    let id = id.to_string();
                    styles.handles.borrow_mut().insert(id.clone(), rules);
                    Ok(HostValue::String(id))
                }
                "adoptStyleSheet" => {
                    let handles = styles.handles.borrow();
                    let rules = handles
                        .get(key)
                        .ok_or_else(|| "stylesheet handle was released".to_owned())?;
                    styles
                        .document
                        .borrow_mut()
                        .document_mut()
                        .append_rules(rules.clone());
                    Ok(HostValue::Undefined)
                }
                "releaseStyleSheet" => {
                    styles.handles.borrow_mut().remove(key);
                    Ok(HostValue::Undefined)
                }
                _ => unreachable!(),
            }
        })?;
    }
    Ok(())
}
