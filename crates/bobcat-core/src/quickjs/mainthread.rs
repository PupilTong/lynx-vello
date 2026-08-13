//! Main-thread (MTS) script execution and the Lynx Element PAPI globals.
//!
//! This is the crate `AGENTS.md` designates for "Lynx host globals": the
//! generic `QuickJS` bridge below stays Lynx-unaware, and the element vocabulary
//! above lives in `lynx-element`. What is assembled here is the realm a
//! `.web.bundle`'s `lepusCode.root` runs in.
//!
//! # What web-core does, and what we reproduce
//!
//! web-core evaluates the main-thread bundle inside a sandboxed same-origin
//! `<iframe>` realm. Before evaluation, `onPageConfigReady` installs the
//! globals with a flat `Object.assign(mtsRealm.globalWindow, …)`; that
//! ordering is guaranteed by the bundle's byte layout, which writes the
//! `Configurations` section before `LepusCode`. The chunk itself is wrapped
//! as
//!
//! ```text
//! //# allFunctionsCalledOnLoad
//! (function(){ "use strict"; const navigator=void 0,postMessage=void 0,window=void 0; CODE
//!  })()
//! ```
//!
//! so a card root is a pure side-effecting script: it assigns `renderPage`,
//! `updatePage`, and friends onto `globalThis`. After evaluation the host runs
//! `processData(initData)` → `renderPage(processedData)` →
//! `__FlushElementTree()`.
//!
//! [`MainThreadRuntime`] follows that shape exactly — a `QuickJS` realm standing
//! in for the iframe, host functions installed before evaluation, the same
//! wrapper, the same post-evaluation sequence.
//!
//! # The realm takes the tree for a batch, and returns it at the flush
//!
//! The [`ElementTree`](lynx_element::ElementTree) changes hands through a
//! [`SharedTree`] slot. A batch's first PAPI mutation takes the tree out;
//! every call after that is a plain `&mut` mutation with no
//! synchronization — the tree's own validation is the single source of
//! every `PapiError`, throwing at the call site. `__FlushElementTree` is
//! the commit boundary: it runs the style + layout commit on the taken
//! tree, puts it back in the slot, and then notifies the presenter through
//! the injected callback. While the tree is away the presenter works from
//! its retained frame, so a half-applied batch is unobservable. A script
//! that opens a batch and returns without flushing gets the tree put back
//! uncommitted at the end of the evaluation — the presenter's
//! `has_uncommitted_mutations` gate keeps that state off the screen.
//!
//! # Recorded limits
//!
//! - **Every `ReactLynx` Snapshot constructor except `__CreateFrame` is installed** — `__CreatePage`,
//!   `__CreateElement`, `__CreateWrapperElement`, `__CreateText`, `__CreateImage`, `__CreateView`,
//!   `__CreateScrollView`, `__CreateRawText`, and `__CreateList` — alongside `__AppendElement`,
//!   `__DropElement`, and `__FlushElementTree` (see `lynx-element`'s crate docs). List construction
//!   consumes only its numeric parent-component argument; callback storage and execution remain
//!   unimplemented. A bundle that reaches for another member gets a `ReferenceError` naming the
//!   missing global, which is the intended failure: a silently wrong render would be worse.
//! - **Element handles cross as opaque JavaScript weak-ref objects.** Each object carries the
//!   element arena id. When `QuickJS` collects it, the realm calls [`ElementTree::drop_element`],
//!   which retires only that Lynx element and its corresponding DOM node. Its surviving descendants
//!   become detached and await their own VM drop notifications. `__DropElement` remains the
//!   explicit early-retirement path through the same operation.
//! - **The non-element main-thread globals are absent** (`lynx`, `SystemInfo`, `__globalProps`,
//!   `_ReportError`, `__OnLifecycleEvent`, `__LoadLepusChunk`, `_I18nResourceTranslation`,
//!   `_AddEventListener`, `__QueryComponent`).
//! - **No background thread.** web-core starts the BTS worker between `processData` and
//!   `renderPage`; there is no second realm here, so `/app-service.js` is never loaded and
//!   `callLepusMethod` has no caller.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use lynx_element::{ElementId, ElementTree, PapiError};
use quickjs_rust_bridge::{self as quickjs, HostFunctionError, HostValue};

use super::{QuickJsInitializationError, QuickJsScriptEngine};
use crate::script::ScriptError;

const MAIN_THREAD_SOURCE_NAME: &str = "main-thread.js";
const BOOT_SOURCE_NAME: &str = "<lynx boot>";
const CREATE_LIST_BINDING_SOURCE_NAME: &str = "<lynx __CreateList binding>";

const WRAPPER_PREFIX: &str = "//# allFunctionsCalledOnLoad\n(function(){ \"use strict\"; \
                              const navigator=void 0,postMessage=void 0,window=void 0; ";
const WRAPPER_SUFFIX: &str = " \n })()\n";

const BOOT_SEQUENCE: &str = r#"(function () {
  "use strict";
  var data = undefined;
  if (typeof processData === "function") {
    data = processData(data);
  }
  if (typeof renderPage !== "function") {
    throw new Error("the main-thread script did not assign globalThis.renderPage");
  }
  renderPage(data);
  __FlushElementTree();
})()"#;

const CREATE_LIST_BINDING: &str = r#"(function () {
  "use strict";
  const createListElement = globalThis.__CreateListElementHost;
  delete globalThis.__CreateListElementHost;
  globalThis.__CreateList = function __CreateList(
    parentComponentUniqueId,
    componentAtIndex,
    enqueueComponent
  ) {
    return createListElement(parentComponentUniqueId);
  };
})()"#;

/// Why running a main-thread script failed.
#[derive(Debug)]
pub struct MainThreadError {
    message: String,
    location: Option<String>,
}

impl MainThreadError {
    fn from_engine(phase: &str, error: &ScriptError) -> Self {
        Self {
            message: format!("{phase}: {}", error.message),
            location: error.location.as_ref().map(|location| {
                let source = location.source.as_deref().unwrap_or("<unknown>");
                match (location.line, location.column) {
                    (Some(line), Some(column)) => format!("{source}:{line}:{column}"),
                    (Some(line), None) => format!("{source}:{line}"),
                    _ => source.to_owned(),
                }
            }),
        }
    }
}

impl fmt::Display for MainThreadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(location) = &self.location {
            write!(formatter, " (at {location})")?;
        }
        Ok(())
    }
}

impl std::error::Error for MainThreadError {}

use crate::engine::SharedTree;

/// The realm's side of the tree hand-off: taken at a batch's first mutation, returned at the flush
/// that commits it.
struct TreeHandle {
    slot: SharedTree,
    taken: Option<ElementTree>,
}

impl TreeHandle {
    fn tree(&mut self) -> &mut ElementTree {
        if self.taken.is_none() {
            self.taken = Some(self.slot.take());
        }
        self.taken
            .as_mut()
            .expect("the batch tree was just ensured")
    }

    fn flush(&mut self) {
        let mut tree = match self.taken.take() {
            Some(tree) => tree,
            None => self.slot.take(),
        };
        tree.flush_element_tree();
        self.slot.put(tree);
    }

    fn release(&mut self) {
        if let Some(tree) = self.taken.take() {
            self.slot.put(tree);
        }
    }
}

impl Drop for TreeHandle {
    fn drop(&mut self) {
        self.release();
    }
}

/// One `QuickJS` realm carrying the Lynx Element PAPI over the tree hand-off slot.
pub struct MainThreadRuntime {
    engine: QuickJsScriptEngine,
    tree: Rc<RefCell<TreeHandle>>,
}

impl fmt::Debug for MainThreadRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadRuntime")
            .finish_non_exhaustive()
    }
}

impl Drop for MainThreadRuntime {
    fn drop(&mut self) {
        // `Engine::run_script` currently retains the last committed tree after boot while the
        // short-lived bootstrap realm goes away. Its teardown must not look like application GC.
        self.engine.realm.clear_js_weak_ref_drop();
        self.tree.borrow_mut().release();
    }
}

impl MainThreadRuntime {
    /// Creates a realm whose Element PAPI takes the tree from `elements` per batch and mutates it
    /// directly, and installs it before any script has run.
    pub fn new(
        elements: SharedTree,
        on_flush: impl Fn() + 'static,
    ) -> Result<Self, QuickJsInitializationError> {
        let mut engine = QuickJsScriptEngine::new()?;
        let tree = install_element_papi(&mut engine.realm, elements, on_flush)
            .map_err(QuickJsInitializationError::from_quickjs)?;
        Ok(Self { engine, tree })
    }

    /// Evaluates a `.web.bundle`'s `lepusCode.root` in web-core's wrapper.
    pub fn evaluate_main_thread_script(&mut self, source: &str) -> Result<(), MainThreadError> {
        let wrapped = format!("{WRAPPER_PREFIX}{source}{WRAPPER_SUFFIX}");
        self.evaluate(
            &wrapped,
            MAIN_THREAD_SOURCE_NAME,
            "evaluating the main-thread script",
        )
    }

    /// Runs web-core's post-evaluation sequence: `processData` (when the bundle defines one), then
    /// `renderPage`, then `__FlushElementTree`.
    pub fn render_page(&mut self) -> Result<(), MainThreadError> {
        self.evaluate(BOOT_SEQUENCE, BOOT_SOURCE_NAME, "rendering the page")
    }

    /// [`Self::evaluate_main_thread_script`] followed by [`Self::render_page`] — the whole boot a
    /// `.web.bundle` gets today.
    pub fn run_main_thread_script(&mut self, source: &str) -> Result<(), MainThreadError> {
        self.evaluate_main_thread_script(source)?;
        self.render_page()
    }

    fn evaluate(&mut self, source: &str, name: &str, phase: &str) -> Result<(), MainThreadError> {
        let result = self
            .engine
            .evaluate_raw(quickjs::EvalSource {
                name: Some(name),
                ..quickjs::EvalSource::new(source)
            })
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine(phase, &error));
        self.tree.borrow_mut().release();
        result
    }
}

fn install_element_papi(
    realm: &mut quickjs::Realm,
    elements: SharedTree,
    on_flush: impl Fn() + 'static,
) -> Result<Rc<RefCell<TreeHandle>>, quickjs::Error> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        slot: elements,
        taken: None,
    }));

    let tree = Rc::clone(&handle);
    realm.set_js_weak_ref_drop(move |id| js_weak_ref_drop(&tree, id));

    let tree = Rc::clone(&handle);
    realm.define_global_function("__CreatePage", 2, move |arguments| {
        let component_id = string_argument("__CreatePage", arguments, 0)?;
        let component_css_id = i32_argument("__CreatePage", arguments, 1)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_page(component_id, component_css_id);
        Ok(js_weak_ref_value(id))
    })?;

    let tree = Rc::clone(&handle);
    realm.define_global_function("__CreateElement", 2, move |arguments| {
        let tag = string_argument("__CreateElement", arguments, 0)?;
        let parent_component = u32_argument("__CreateElement", arguments, 1)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_element(tag, parent_component)
            .map_err(papi_error)?;
        Ok(js_weak_ref_value(id))
    })?;

    define_parent_element_constructor(
        realm,
        &handle,
        "__CreateWrapperElement",
        ElementTree::create_wrapper_element,
    )?;
    define_parent_element_constructor(realm, &handle, "__CreateText", ElementTree::create_text)?;
    define_parent_element_constructor(realm, &handle, "__CreateImage", ElementTree::create_image)?;
    define_parent_element_constructor(realm, &handle, "__CreateView", ElementTree::create_view)?;
    define_parent_element_constructor(
        realm,
        &handle,
        "__CreateScrollView",
        ElementTree::create_scroll_view,
    )?;

    let tree = Rc::clone(&handle);
    realm.define_global_function("__CreateRawText", 1, move |arguments| {
        let text = string_argument("__CreateRawText", arguments, 0)?;
        let id = tree.borrow_mut().tree().create_raw_text(text);
        Ok(js_weak_ref_value(id))
    })?;

    // ReactLynx passes callback functions and an info object after the parent component id. Those
    // values stay in JavaScript until list callback execution exists; the leaf host binding sees
    // only the primitive argument it owns.
    let tree = Rc::clone(&handle);
    realm.define_global_function("__CreateListElementHost", 1, move |arguments| {
        let parent_component = u32_argument("__CreateList", arguments, 0)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_list(parent_component)
            .map_err(papi_error)?;
        Ok(js_weak_ref_value(id))
    })?;
    realm
        .evaluate(
            quickjs::EvalSource {
                name: Some(CREATE_LIST_BINDING_SOURCE_NAME),
                ..quickjs::EvalSource::new(CREATE_LIST_BINDING)
            },
            quickjs::EvalOptions::default(),
        )
        .map(|_| ())?;

    let tree = Rc::clone(&handle);
    realm.define_global_function("__AppendElement", 2, move |arguments| {
        let parent = element_argument("__AppendElement", arguments, 0)?;
        let child = element_argument("__AppendElement", arguments, 1)?;
        let appended = tree
            .borrow_mut()
            .tree()
            .append_element(parent, child)
            .map_err(papi_error)?;
        Ok(js_weak_ref_value(appended))
    })?;

    let tree = Rc::clone(&handle);
    realm.define_global_function("__DropElement", 1, move |arguments| {
        let id = element_argument("__DropElement", arguments, 0)?;
        match tree.borrow_mut().tree().drop_element(id) {
            Ok(()) | Err(PapiError::UnknownElement(_)) => {}
            Err(error) => return Err(papi_error(error)),
        }
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(&handle);
    realm.define_global_function("__FlushElementTree", 0, move |_arguments| {
        tree.borrow_mut().flush();
        on_flush();
        Ok(HostValue::Undefined)
    })?;

    Ok(handle)
}

type ParentElementConstructor = fn(&mut ElementTree, ElementId) -> Result<ElementId, PapiError>;

fn define_parent_element_constructor(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
    name: &'static str,
    constructor: ParentElementConstructor,
) -> Result<(), quickjs::Error> {
    let tree = Rc::clone(handle);
    realm.define_global_function(name, 1, move |arguments| {
        let parent_component = u32_argument(name, arguments, 0)?;
        let id = constructor(tree.borrow_mut().tree(), parent_component).map_err(papi_error)?;
        Ok(js_weak_ref_value(id))
    })
}

fn js_weak_ref_value(id: ElementId) -> HostValue {
    HostValue::JsWeakRef(id)
}

fn js_weak_ref_drop(tree: &Rc<RefCell<TreeHandle>>, id: ElementId) {
    match tree.borrow_mut().tree().drop_element(id) {
        Ok(()) | Err(PapiError::UnknownElement(_) | PapiError::CannotRemovePage) => {}
        Err(error) => debug_assert!(false, "unexpected weak-ref drop failure: {error}"),
    }
}

fn papi_error(error: impl fmt::Display) -> HostFunctionError {
    HostFunctionError::new(error.to_string())
}

fn argument(arguments: &[HostValue], index: usize) -> &HostValue {
    arguments.get(index).unwrap_or(&HostValue::Undefined)
}

fn u32_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<u32, HostFunctionError> {
    let HostValue::Number(value) = *argument(arguments, index) else {
        return Err(HostFunctionError::new(format!(
            "{function} expects a number for argument {index}"
        )));
    };
    if !value.is_finite() || value.fract() != 0.0 || value < 0.0 || value > f64::from(u32::MAX) {
        return Err(HostFunctionError::new(format!(
            "{function} expects an unsigned 32-bit integer for argument {index}, got {value}"
        )));
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the range and integrality checks above make this exact"
    )]
    Ok(value as u32)
}

fn i32_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<i32, HostFunctionError> {
    match *argument(arguments, index) {
        HostValue::Undefined | HostValue::Null => Ok(0),
        HostValue::Number(value)
            if value.is_finite()
                && value.fract() == 0.0
                && value >= f64::from(i32::MIN)
                && value <= f64::from(i32::MAX) =>
        {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the range and integrality checks above make this exact"
            )]
            Ok(value as i32)
        }
        _ => Err(HostFunctionError::new(format!(
            "{function} expects an integer for argument {index}"
        ))),
    }
}

fn string_argument<'a>(
    function: &str,
    arguments: &'a [HostValue],
    index: usize,
) -> Result<&'a str, HostFunctionError> {
    match argument(arguments, index) {
        HostValue::String(value) => Ok(value),
        HostValue::Undefined | HostValue::Null => Ok(""),
        _ => Err(HostFunctionError::new(format!(
            "{function} expects a string for argument {index}"
        ))),
    }
}

fn element_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<ElementId, HostFunctionError> {
    let HostValue::JsWeakRef(id) = *argument(arguments, index) else {
        return Err(HostFunctionError::new(format!(
            "{function} expects a JavaScript element weak reference for argument {index}"
        )));
    };
    Ok(id)
}
