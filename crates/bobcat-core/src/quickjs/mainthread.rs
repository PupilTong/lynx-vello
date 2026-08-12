//! Main-thread (MTS) script execution and the Lynx Element PAPI facade.
//!
//! This is the crate `AGENTS.md` designates for the Lynx host API: the
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
//! - **Six numeric host methods live on `globalThis.bobcat`** — `create_page`, `create_view`,
//!   `append_element`, `remove_element`, `drop_element`, and `flush_element_tree`. A JavaScript
//!   facade installs the matching web-core globals before bundle code runs.
//! - **Element handles are JavaScript-owned wrapper objects.** A weak arena returns the same live
//!   wrapper for one `ElementId`; a `FinalizationRegistry` calls `bobcat.drop_element(id)` after
//!   the last wrapper becomes unreachable. Its direct children become detached roots and remain
//!   live while their own wrappers are reachable. There is no parallel native lease count.
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
const ELEMENT_WRAPPER_SOURCE_NAME: &str = "<bobcat element wrappers>";
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

const ELEMENT_WRAPPER_API: &str = r#"(function () {
  "use strict";

  const native = globalThis.bobcat;
  const wrappers = [];
  const elementEntries = new WeakMap();
  const pendingFinalizers = [];
  const resolvedPromise = Promise.resolve();
  const promiseThen = Promise.prototype.then;
  let finalizerDrainScheduled = false;
  let batchOpen = false;

  function flush() {
    try {
      return native.flush_element_tree();
    } finally {
      batchOpen = false;
    }
  }

  function drainFinalizers() {
    finalizerDrainScheduled = false;
    const queued = pendingFinalizers.splice(0);
    const batchWasOpen = batchOpen;
    let dropped = false;

    for (const entry of queued) {
      // A wrapper for the same id may have been recreated after this cleanup
      // was queued. That newer live owner supersedes the stale callback.
      if (wrappers[entry.id] !== entry) {
        continue;
      }
      wrappers[entry.id] = undefined;
      native.drop_element(entry.id);
      dropped = true;
    }

    if (dropped) {
      batchOpen = true;
      // Do not make an earlier abandoned application batch visible merely
      // because a FinalizationRegistry cleanup happened later.
      if (!batchWasOpen) {
        flush();
      }
    }
  }

  const finalizer = new FinalizationRegistry(function (entry) {
    if (wrappers[entry.id] !== entry) {
      return;
    }
    pendingFinalizers.push(entry);
    if (!finalizerDrainScheduled) {
      finalizerDrainScheduled = true;
      promiseThen.call(resolvedPromise, drainFinalizers);
    }
  });

  function wrapperFor(id) {
    const current = wrappers[id];
    if (current !== undefined) {
      const wrapper = current.reference.deref();
      if (wrapper !== undefined) {
        return wrapper;
      }
    }

    const wrapper = {};
    Object.defineProperty(wrapper, "ElementId", {
      value: id,
      enumerable: true,
    });
    const entry = { id: id, reference: new WeakRef(wrapper) };
    wrappers[id] = entry;
    elementEntries.set(wrapper, entry);
    // The page is permanent in ElementTree and therefore has no GC drop.
    if (id !== 1) {
      finalizer.register(wrapper, entry, wrapper);
    }
    return wrapper;
  }

  function elementEntry(functionName, value, index) {
    const entry = elementEntries.get(value);
    if (entry === undefined) {
      throw new TypeError(
        functionName + " expects an element wrapper for argument " + index
      );
    }
    return entry;
  }

  globalThis.__CreatePage = function __CreatePage(componentId, componentCssId) {
    const id = native.create_page(componentId, componentCssId);
    batchOpen = true;
    return wrapperFor(id);
  };

  globalThis.__CreateView = function __CreateView(parentComponentId) {
    const id = native.create_view(parentComponentId);
    batchOpen = true;
    return wrapperFor(id);
  };

  globalThis.__AppendElement = function __AppendElement(parent, child) {
    const parentEntry = elementEntry("__AppendElement", parent, 0);
    const childEntry = elementEntry("__AppendElement", child, 1);
    native.append_element(parentEntry.id, childEntry.id);
    batchOpen = true;
    return child;
  };

  globalThis.__RemoveElement = function __RemoveElement(parent, child) {
    const parentEntry = elementEntry("__RemoveElement", parent, 0);
    const childEntry = elementEntry("__RemoveElement", child, 1);
    native.remove_element(parentEntry.id, childEntry.id);
    batchOpen = true;
    return child;
  };

  globalThis.__DropElement = function __DropElement(element) {
    const entry = elementEntry("__DropElement", element, 0);
    native.drop_element(entry.id);
    batchOpen = true;
    finalizer.unregister(element);
    if (wrappers[entry.id] === entry) {
      wrappers[entry.id] = undefined;
    }
  };

  globalThis.__FlushElementTree = function __FlushElementTree() {
    return flush();
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

    /// Runs enough `QuickJS` collection passes to discover cyclic weak targets,
    /// then executes the queued `FinalizationRegistry` jobs. This is primarily
    /// an embedder memory-pressure seam.
    pub fn collect_garbage(&mut self) -> Result<(), MainThreadError> {
        let result = self
            .engine
            .collect_garbage_and_run_cleanup_jobs()
            .map_err(|error| MainThreadError::from_engine("collecting garbage", &error));
        self.tree.borrow_mut().release();
        result
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

    let bobcat = realm.evaluate(
        quickjs::EvalSource {
            name: Some(ELEMENT_WRAPPER_SOURCE_NAME),
            ..quickjs::EvalSource::new("Object.create(null)")
        },
        quickjs::EvalOptions::default(),
    )?;

    let tree = Rc::clone(&handle);
    let function = realm.function("create_page", 2, move |arguments| {
        let component_id = string_argument("bobcat.create_page", arguments, 0)?;
        let component_css_id = i32_argument("bobcat.create_page", arguments, 1)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_page(component_id, component_css_id);
        Ok(element_id_value(id))
    })?;
    realm.set_property(&bobcat, "create_page", &function)?;

    let tree = Rc::clone(&handle);
    let function = realm.function("create_view", 1, move |arguments| {
        let parent_component = u32_argument("bobcat.create_view", arguments, 0)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_view(parent_component)
            .map_err(papi_error)?;
        Ok(element_id_value(id))
    })?;
    realm.set_property(&bobcat, "create_view", &function)?;

    let tree = Rc::clone(&handle);
    let function = realm.function("append_element", 2, move |arguments| {
        let parent = element_argument("bobcat.append_element", arguments, 0)?;
        let child = element_argument("bobcat.append_element", arguments, 1)?;
        let appended = tree
            .borrow_mut()
            .tree()
            .append_element(parent, child)
            .map_err(papi_error)?;
        Ok(element_id_value(appended))
    })?;
    realm.set_property(&bobcat, "append_element", &function)?;

    let tree = Rc::clone(&handle);
    let function = realm.function("remove_element", 2, move |arguments| {
        let parent = element_argument("bobcat.remove_element", arguments, 0)?;
        let child = element_argument("bobcat.remove_element", arguments, 1)?;
        let removed = tree
            .borrow_mut()
            .tree()
            .remove_element(parent, child)
            .map_err(papi_error)?;
        Ok(element_id_value(removed))
    })?;
    realm.set_property(&bobcat, "remove_element", &function)?;

    let tree = Rc::clone(&handle);
    let function = realm.function("drop_element", 1, move |arguments| {
        let id = element_argument("bobcat.drop_element", arguments, 0)?;
        match tree.borrow_mut().tree().drop_element(id) {
            Ok(()) | Err(PapiError::UnknownElement(_)) => {}
            Err(error) => return Err(papi_error(error)),
        }
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&bobcat, "drop_element", &function)?;

    let tree = Rc::clone(&handle);
    let function = realm.function("flush_element_tree", 0, move |_arguments| {
        tree.borrow_mut().flush();
        on_flush();
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&bobcat, "flush_element_tree", &function)?;

    let global = realm.global_object()?;
    realm.set_property(&global, "bobcat", &bobcat)?;
    realm.evaluate(
        quickjs::EvalSource {
            name: Some(ELEMENT_WRAPPER_SOURCE_NAME),
            ..quickjs::EvalSource::new(ELEMENT_WRAPPER_API)
        },
        quickjs::EvalOptions::default(),
    )?;

    Ok(handle)
}

fn element_id_value(id: ElementId) -> HostValue {
    HostValue::Number(f64::from(id))
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
    let id = u32_argument(function, arguments, index)?;
    (id != 0).then_some(id).ok_or_else(|| {
        HostFunctionError::new(format!(
            "{function} expects an element id for argument {index}, got the null handle 0"
        ))
    })
}
