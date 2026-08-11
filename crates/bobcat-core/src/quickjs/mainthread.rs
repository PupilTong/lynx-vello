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
//! - **Six Element PAPI members are installed** — `__CreatePage`, `__CreateView`,
//!   `__AppendElement`, `__RemoveElement`, `__DropElement`, `__FlushElementTree` (see
//!   `lynx-element`'s crate docs). A bundle that reaches for anything else gets a `ReferenceError`
//!   naming the missing global, which is the intended failure: a silently wrong render would be
//!   worse.
//! - **Element handles are opaque Rust-backed JavaScript objects.** Each object carries one
//!   `ElementId`; its `QuickJS` class finalizer queues that id, and the next safe batch boundary
//!   retires only that element. Its direct children become detached roots and remain live while
//!   their own wrappers are reachable. JavaScript reachability is the ownership model — there is no
//!   parallel native lease count. The permanent page wrapper is the sole cached handle because
//!   `__CreatePage` is idempotent and must return the same object.
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
/// Source name for the tiny JavaScript facade that preserves PAPI return-value
/// identity without exposing the bridge's private native helper names.
const ELEMENT_OBJECT_API_SOURCE_NAME: &str = "<element object API>";

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

/// `__CreatePage` is the one idempotent creator, so one closure slot preserves
/// its object identity without a general `ElementId -> wrapper` map.
/// `__AppendElement` and `__RemoveElement` return their exact child argument,
/// matching `appendChild`/`removeChild`, rather than manufacturing a second
/// wrapper for the same native element.
const ELEMENT_OBJECT_API: &str = r#"(function () {
  "use strict";
  const createPage = globalThis.__BobcatCreatePage;
  const appendElement = globalThis.__BobcatAppendElement;
  const removeElement = globalThis.__BobcatRemoveElement;
  let page;
  globalThis.__CreatePage = function __CreatePage(componentID, componentCSSID) {
    if (page === undefined) {
      page = createPage(componentID, componentCSSID);
    }
    return page;
  };
  globalThis.__AppendElement = function __AppendElement(parent, child) {
    appendElement(parent, child);
    return child;
  };
  globalThis.__RemoveElement = function __RemoveElement(parent, child) {
    removeElement(parent, child);
    return child;
  };
  delete globalThis.__BobcatCreatePage;
  delete globalThis.__BobcatAppendElement;
  delete globalThis.__BobcatRemoveElement;
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
    /// Whether the tree returned to `slot` carries an abandoned, uncommitted
    /// batch from an earlier evaluation. A later GC-only mutation must not
    /// accidentally turn that half-applied batch into a visible commit.
    returned_tree_uncommitted: bool,
    /// Payloads queued by the `QuickJS` class finalizer. The finalizer itself
    /// performs no tree work and invokes no application callback.
    releases: quickjs::HostObjectReleaseQueue,
}

impl TreeHandle {
    fn tree(&mut self) -> &mut ElementTree {
        if self.taken.is_none() {
            let tree = self.slot.take();
            debug_assert_eq!(
                tree.has_uncommitted_mutations(),
                self.returned_tree_uncommitted,
                "the hand-off metadata must match the returned tree"
            );
            self.taken = Some(tree);
        }
        self.taken
            .as_mut()
            .expect("the batch tree was just ensured")
    }

    fn drain_released_elements(&mut self) -> bool {
        let mut changed = false;
        for id in self.releases.drain() {
            // The document element is permanent and its one wrapper is rooted
            // by the idempotent `__CreatePage` facade until realm teardown.
            if id == 1 {
                continue;
            }
            match self.tree().drop_element(id) {
                Ok(()) => changed = true,
                Err(PapiError::UnknownElement(_)) => {}
                Err(error) => {
                    debug_assert!(false, "unexpected GC element release error: {error}");
                }
            }
        }
        changed
    }

    fn flush(&mut self) {
        self.drain_released_elements();
        let mut tree = match self.taken.take() {
            Some(tree) => tree,
            None => self.slot.take(),
        };
        tree.flush_element_tree();
        self.returned_tree_uncommitted = false;
        self.slot.put(tree);
    }

    fn release(&mut self) {
        if let Some(tree) = self.taken.take() {
            self.returned_tree_uncommitted = tree.has_uncommitted_mutations();
            self.slot.put(tree);
        }
    }

    /// Finishes one evaluation and reports whether a GC-only batch was
    /// committed. Releases queued after the script's final explicit flush need
    /// their own commit; releases joining an already-open, abandoned batch do
    /// not make that half-applied batch observable.
    fn finish_evaluation(&mut self) -> bool {
        let batch_was_open = self.taken.is_some();
        let earlier_batch_was_abandoned = !batch_was_open && self.returned_tree_uncommitted;
        let changed = self.drain_released_elements();
        if changed && !batch_was_open && !earlier_batch_was_abandoned {
            self.flush();
            true
        } else {
            self.release();
            false
        }
    }
}

impl Drop for TreeHandle {
    fn drop(&mut self) {
        // `Realm` teardown finalizes every remaining wrapper before the host
        // closures release their last `Rc<TreeHandle>`. Drain those final ids
        // and always return a taken tree to its hand-off slot.
        if self.drain_released_elements() {
            self.flush();
        } else {
            self.release();
        }
    }
}

/// One `QuickJS` realm carrying the Lynx Element PAPI over the tree hand-off slot.
pub struct MainThreadRuntime {
    engine: QuickJsScriptEngine,
    tree: Rc<RefCell<TreeHandle>>,
    on_flush: Rc<dyn Fn()>,
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
        let on_flush: Rc<dyn Fn()> = Rc::new(on_flush);
        let tree = install_element_papi(&mut engine.realm, elements, Rc::clone(&on_flush))
            .map_err(QuickJsInitializationError::from_quickjs)?;
        Ok(Self {
            engine,
            tree,
            on_flush,
        })
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

    /// Runs a full `QuickJS` collection and commits any element releases it
    /// discovers. This is primarily an embedder memory-pressure seam; ordinary
    /// acyclic wrappers are finalized by `QuickJS` reference counting without an
    /// explicit collection.
    pub fn collect_garbage(&mut self) {
        self.engine.realm.run_gc();
        if self.tree.borrow_mut().finish_evaluation() {
            (self.on_flush)();
        }
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
        if self.tree.borrow_mut().finish_evaluation() {
            (self.on_flush)();
        }
        result
    }
}

fn install_element_papi(
    realm: &mut quickjs::Realm,
    elements: SharedTree,
    on_flush: Rc<dyn Fn()>,
) -> Result<Rc<RefCell<TreeHandle>>, quickjs::Error> {
    let releases = realm.host_object_release_queue();
    let returned_tree_uncommitted = elements.tree().has_uncommitted_mutations();
    let handle = Rc::new(RefCell::new(TreeHandle {
        slot: elements,
        taken: None,
        returned_tree_uncommitted,
        releases,
    }));

    let tree = Rc::clone(&handle);
    realm.define_global_function("__BobcatCreatePage", 2, move |arguments| {
        let component_id = string_argument("__CreatePage", arguments, 0)?;
        let component_css_id = i32_argument("__CreatePage", arguments, 1)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_page(component_id, component_css_id);
        Ok(element_value(id))
    })?;

    let tree = Rc::clone(&handle);
    realm.define_global_function("__CreateView", 1, move |arguments| {
        let parent_component = u32_argument("__CreateView", arguments, 0)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_view(parent_component)
            .map_err(papi_error)?;
        Ok(element_value(id))
    })?;

    let tree = Rc::clone(&handle);
    realm.define_global_function("__BobcatAppendElement", 2, move |arguments| {
        let parent = element_argument("__AppendElement", arguments, 0)?;
        let child = element_argument("__AppendElement", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .append_element(parent, child)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // The private native half of `__RemoveElement(parent, child)`. It only
    // detaches the direct child; neither wrapper nor native element is
    // retired. The JS facade returns the exact child argument.
    let tree = Rc::clone(&handle);
    realm.define_global_function("__BobcatRemoveElement", 2, move |arguments| {
        let parent = element_argument("__RemoveElement", arguments, 0)?;
        let child = element_argument("__RemoveElement", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .remove_element(parent, child)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
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
        (on_flush)();
        Ok(HostValue::Undefined)
    })?;

    // Install the public page/append/remove facades only after their private
    // native functions exist, then remove the private names from the global
    // object.
    realm.evaluate(
        quickjs::EvalSource {
            name: Some(ELEMENT_OBJECT_API_SOURCE_NAME),
            ..quickjs::EvalSource::new(ELEMENT_OBJECT_API)
        },
        quickjs::EvalOptions::default(),
    )?;

    Ok(handle)
}

fn element_value(id: ElementId) -> HostValue {
    HostValue::Object(quickjs::HostObject::new(id))
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
    match argument(arguments, index) {
        HostValue::Object(object) => Ok(object.payload()),
        _ => Err(HostFunctionError::new(format!(
            "{function} expects an element object for argument {index}"
        ))),
    }
}
