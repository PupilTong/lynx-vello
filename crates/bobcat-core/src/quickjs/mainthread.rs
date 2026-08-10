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
//! # The prelude
//!
//! Most PAPI members are host functions, one per member. Four are not, because
//! their web-core signatures carry an array, a plain object, or an optional
//! trailing record, and the `QuickJS` host boundary below is primitives-only by
//! design. Those are assembled in JavaScript by [`PRELUDE`], evaluated in the
//! realm before the bundle, over primitive-shaped host builtins that the
//! prelude then deletes from the global object.
//!
//! This is not a workaround bolted onto web-core's design — it *is* web-core's
//! design. Its PAPI members are JavaScript closures over a Rust/WASM context
//! (`createElementAPI.ts`), and the reshaping the prelude does here
//! (`Object.keys` walks, scalar-or-array coercion, camelCase-to-kebab-case) is
//! the same reshaping those closures do before they reach Rust.
//!
//! # Recorded limits
//!
//! - **The installed subset is the one a compiled `ReactLynx` app calls** — see `lynx-element`'s
//!   crate docs for the member table and for the families deliberately left out (events, lists,
//!   worklets, gestures, animation, selector queries, element templates). A bundle that reaches for
//!   anything else gets a `ReferenceError` naming the missing global, which is the intended
//!   failure: a silently wrong render would be worse.
//! - **Element handles cross as `u32` unique-id numbers**, not element objects. web-core hands out
//!   `HTMLElement`s decorated with a unique-id symbol; the number is the same identity
//!   `__GetElementUniqueID` reports there. A number cannot be held weakly, so reclamation cannot be
//!   inferred the way web-core's `WeakRef` sweep infers it — the script announces it through
//!   `__DropElement`, which is why this runtime carries that global and web-core has no equivalent.
//!   Registering each new handle with a realm-side `FinalizationRegistry` is the `__Create*`
//!   members' side of that contract and is not wired up yet, so today an element the script forgets
//!   about is retained until it is dropped explicitly.
//! - **A dataset value that is a plain object is stored as its JSON text.** web-core keeps the live
//!   object in its side table and JSON-encodes only the mirrored `data-*` attribute; the
//!   primitives-only boundary collapses the two.
//! - **The non-element main-thread globals are absent** (`lynx`, `SystemInfo`, `__globalProps`,
//!   `_ReportError`, `__OnLifecycleEvent`, `__LoadLepusChunk`, `_I18nResourceTranslation`,
//!   `_AddEventListener`, `__QueryComponent`).
//! - **No background thread.** web-core starts the BTS worker between `processData` and
//!   `renderPage`; there is no second realm here, so `/app-service.js` is never loaded and
//!   `callLepusMethod` has no caller.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use lynx_element::{ElementId, ElementTree, NO_ELEMENT, PapiError, PapiValue};
use quickjs_rust_bridge::{self as quickjs, HostFunctionError, HostValue};

use super::{QuickJsInitializationError, QuickJsScriptEngine};
use crate::script::ScriptError;

/// The source name `QuickJS` reports for the main-thread bundle.
const MAIN_THREAD_SOURCE_NAME: &str = "main-thread.js";
/// The source name for the boot sequence this module drives.
const BOOT_SOURCE_NAME: &str = "<lynx boot>";

/// The exact wrapper web-core's decode worker builds around every lepus chunk.
const WRAPPER_PREFIX: &str = "//# allFunctionsCalledOnLoad\n(function(){ \"use strict\"; \
                              const navigator=void 0,postMessage=void 0,window=void 0; ";
const WRAPPER_SUFFIX: &str = " \n })()\n";

/// web-core's `onMTSScriptsExecuted`, transliterated.
///
/// `typeof` is used rather than a bare reference because the wrapper above runs
/// in strict mode, where reading an undeclared identifier is a `ReferenceError`
/// — and a bundle is free not to define `processData`.
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

/// The realm's side of the tree hand-off: taken at a batch's first
/// mutation, returned at the flush that commits it.
struct TreeHandle {
    slot: SharedTree,
    /// The tree while a batch is open on this thread.
    taken: Option<ElementTree>,
}

impl TreeHandle {
    /// The tree for one PAPI mutation, opening a batch if none is open.
    /// Taking blocks only for the presenting side's brief borrows.
    fn tree(&mut self) -> &mut ElementTree {
        if self.taken.is_none() {
            self.taken = Some(self.slot.take());
        }
        self.taken
            .as_mut()
            .expect("the batch tree was just ensured")
    }

    /// The commit boundary: style + layout on the taken tree, then the
    /// hand-back. A flush with no prior mutation still commits — a flush is
    /// a style + layout pass even with nothing recorded.
    fn flush(&mut self) {
        let mut tree = match self.taken.take() {
            Some(tree) => tree,
            None => self.slot.take(),
        };
        tree.flush_element_tree();
        self.slot.put(tree);
    }

    /// Returns the tree unconditionally — the end-of-evaluation backstop
    /// for a script that opened a batch and never flushed. The returned
    /// tree still reports uncommitted mutations, which keeps the abandoned
    /// batch off the screen.
    fn release(&mut self) {
        if let Some(tree) = self.taken.take() {
            self.slot.put(tree);
        }
    }
}

/// One `QuickJS` realm carrying the Lynx Element PAPI over the tree
/// hand-off slot.
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
    /// Creates a realm whose Element PAPI takes the tree from `elements`
    /// per batch and mutates it directly, and installs it before any script
    /// has run. `on_flush` runs after every committed `__FlushElementTree`,
    /// once the tree is back in its slot — the seam a presenter uses to
    /// learn a committed frame is available.
    pub fn new(
        elements: SharedTree,
        on_flush: impl Fn() + 'static,
    ) -> Result<Self, QuickJsInitializationError> {
        let mut engine = QuickJsScriptEngine::new()?;
        let tree = install_element_papi(&mut engine.realm, elements, on_flush)
            .map_err(QuickJsInitializationError::from_quickjs)?;
        let mut runtime = Self { engine, tree };
        runtime
            .evaluate(PRELUDE, PRELUDE_SOURCE_NAME, "installing the Element PAPI")
            .map_err(|error| {
                QuickJsInitializationError::from_message(format!(
                    "the Element PAPI prelude failed: {error}"
                ))
            })?;
        Ok(runtime)
    }

    /// Evaluates a `.web.bundle`'s `lepusCode.root` in web-core's wrapper.
    ///
    /// A card root works purely by side effect: it assigns its entry points
    /// onto `globalThis`. Nothing is rendered yet — call [`Self::render_page`].
    pub fn evaluate_main_thread_script(&mut self, source: &str) -> Result<(), MainThreadError> {
        let wrapped = format!("{WRAPPER_PREFIX}{source}{WRAPPER_SUFFIX}");
        self.evaluate(
            &wrapped,
            MAIN_THREAD_SOURCE_NAME,
            "evaluating the main-thread script",
        )
    }

    /// Runs web-core's post-evaluation sequence: `processData` (when the
    /// bundle defines one), then `renderPage`, then `__FlushElementTree`.
    pub fn render_page(&mut self) -> Result<(), MainThreadError> {
        self.evaluate(BOOT_SEQUENCE, BOOT_SOURCE_NAME, "rendering the page")
    }

    /// [`Self::evaluate_main_thread_script`] followed by [`Self::render_page`]
    /// — the whole boot a `.web.bundle` gets today.
    pub fn run_main_thread_script(&mut self, source: &str) -> Result<(), MainThreadError> {
        self.evaluate_main_thread_script(source)?;
        self.render_page()
    }

    /// Evaluates through the engine's own checkpoint state machine.
    ///
    /// web-core's MTS realm is a browser realm, where promise jobs queued
    /// during evaluation run before control reaches the host again — so the
    /// microtask drain is not optional. Going through `evaluate_raw` rather
    /// than driving the realm directly is what keeps this wrapper's job
    /// ordering identical to the crate's `ScriptEngine` impl, including
    /// resuming a checkpoint that previously hit the per-call job limit.
    fn evaluate(&mut self, source: &str, name: &str, phase: &str) -> Result<(), MainThreadError> {
        let result = self
            .engine
            .evaluate_raw(quickjs::EvalSource {
                name: Some(name),
                ..quickjs::EvalSource::new(source)
            })
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine(phase, &error));
        // Whatever the script did — flushed, failed, or abandoned a batch —
        // the tree must be back in its slot when control returns to the
        // host.
        self.tree.borrow_mut().release();
        result
    }
}

/// The source name `QuickJS` reports for [`PRELUDE`].
const PRELUDE_SOURCE_NAME: &str = "<lynx element papi>";

/// The JavaScript half of the Element PAPI.
///
/// Four members carry an array, a plain object, or an optional trailing
/// record, none of which crosses the primitives-only host boundary. Each is
/// assembled here over a primitive-shaped builtin that this script then
/// removes from the global object, so the only `__`-prefixed globals a bundle
/// can see are real PAPI member names.
///
/// The reshaping is web-core's, transliterated:
/// - the `Object.keys` walks match `__SetInlineStyles`' and `__SetDataset`'s record branches
///   (`createElementAPI.ts:361-374`, `:422-425`);
/// - `JSON.stringify` on an object dataset value matches `:431`;
/// - the scalar-or-array `__SetCSSId` argument matches the native declaration `ReactLynx` compiles
///   against (`element-api.d.ts:236`), which is wider than web-core's array-only type;
/// - the trailing `info` record every `__Create*` member accepts is a `ui-source-map` annotation
///   web-core also ignores (`createElementAPI.ts` destructures only the leading parameters), so it
///   is dropped here.
const PRELUDE: &str = r#"(function () {
  "use strict";
  var global = globalThis;

  // Drop arguments past the ones the host member declares. Every `__Create*`
  // member takes an optional trailing `info` record under `ui-source-map`.
  function clampArity(name, arity) {
    var host = global[name];
    global[name] = function () {
      return host.apply(null, Array.prototype.slice.call(arguments, 0, arity));
    };
  }
  clampArity("__CreatePage", 2);
  clampArity("__CreateElement", 2);
  clampArity("__CreateView", 1);
  clampArity("__CreateText", 1);
  clampArity("__CreateImage", 1);
  clampArity("__CreateScrollView", 1);
  clampArity("__CreateWrapperElement", 1);
  clampArity("__CreateFrame", 1);

  // `__SetCSSId(elements, cssId, entryName)` — one element or an array of
  // them; a nullish id means "no scope", which web-core spells 0.
  var setCssIdOfElement = global.__SetCSSIdOfElement;
  delete global.__SetCSSIdOfElement;
  global.__SetCSSId = function (elements, cssId, entryName) {
    var id = cssId == null ? 0 : cssId;
    if (typeof elements === "number") {
      setCssIdOfElement(elements, id, entryName);
      return;
    }
    for (var index = 0; index < elements.length; index += 1) {
      setCssIdOfElement(elements[index], id, entryName);
    }
  };

  // `__AddDataset(element, key, value)` — an object value is stored as its
  // JSON text, which is what web-core writes to the mirrored `data-*`
  // attribute.
  var addDatasetEntry = global.__AddDatasetEntry;
  delete global.__AddDatasetEntry;
  global.__AddDataset = function (element, key, value) {
    addDatasetEntry(element, key, encodeDataValue(value));
  };

  // `__SetDataset(element, dataset)` — a whole-map replace, not a merge.
  var clearDataset = global.__ClearDataset;
  delete global.__ClearDataset;
  global.__SetDataset = function (element, dataset) {
    clearDataset(element);
    if (dataset == null) return;
    var keys = Object.keys(dataset);
    for (var index = 0; index < keys.length; index += 1) {
      addDatasetEntry(element, keys[index], encodeDataValue(dataset[keys[index]]));
    }
  };

  function encodeDataValue(value) {
    if (value !== null && typeof value === "object") return JSON.stringify(value);
    if (typeof value === "function" || typeof value === "symbol") return String(value);
    return value;
  }

  // `__SetInlineStyles(element, value)` — a declaration string, a property
  // record, or nothing at all.
  var setInlineStyleText = global.__SetInlineStyleText;
  delete global.__SetInlineStyleText;
  global.__SetInlineStyles = function (element, value) {
    if (value == null || value === "") {
      setInlineStyleText(element, null);
      return;
    }
    if (typeof value !== "object") {
      setInlineStyleText(element, String(value));
      return;
    }
    var text = "";
    var keys = Object.keys(value);
    for (var index = 0; index < keys.length; index += 1) {
      var declared = value[keys[index]];
      if (declared == null) continue;
      text += hyphenate(keys[index]) + ":" + declared + ";";
    }
    setInlineStyleText(element, text === "" ? null : text);
  };

  // web-core accepts both `marginTop` and `margin-top` as record keys.
  function hyphenate(property) {
    if (property.indexOf("--") === 0) return property;
    return property.replace(/[A-Z]/g, function (letter) {
      return "-" + letter.toLowerCase();
    });
  }

  // `__GetClasses(element)` — an ordered array, not a set.
  var getClassText = global.__GetClassText;
  delete global.__GetClassText;
  global.__GetClasses = function (element) {
    var text = getClassText(element);
    if (!text) return [];
    return text.split(" ");
  };

  // `__GetChildren(element)` — a real Array of handles.
  var childCount = global.__ChildCount;
  var childAt = global.__ChildAt;
  delete global.__ChildCount;
  delete global.__ChildAt;
  global.__GetChildren = function (element) {
    var count = childCount(element);
    var children = [];
    for (var index = 0; index < count; index += 1) {
      children.push(childAt(element, index));
    }
    return children;
  };
})()"#;

/// Installs the Element PAPI onto the realm's global object, returning the
/// hand-off handle the runtime releases at every evaluation boundary.
///
/// web-core does the equivalent with one `Object.assign` of a closure
/// literal; each closure here reaches the batch's taken tree through the
/// same handle. Validation is the tree's own — a bad handle throws at the
/// call site.
fn install_element_papi(
    realm: &mut quickjs::Realm,
    elements: SharedTree,
    on_flush: impl Fn() + 'static,
) -> Result<Rc<RefCell<TreeHandle>>, quickjs::Error> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        slot: elements,
        taken: None,
    }));

    install_creation(realm, &handle)?;
    install_structure(realm, &handle)?;
    install_disposal(realm, &handle)?;
    install_queries(realm, &handle)?;
    install_properties(realm, &handle)?;
    install_styling(realm, &handle)?;
    install_dataset(realm, &handle)?;
    install_scope(realm, &handle)?;

    // `__FlushElementTree()` — the single commit boundary: the style + layout
    // commit runs on the taken tree, the tree goes back in its slot, and the
    // presenter is notified. An empty batch still commits — a flush is a
    // style + layout pass even with nothing recorded. web-core ignores the
    // optional sub-tree and options arguments on the web target too.
    let tree = Rc::clone(&handle);
    realm.define_global_function("__FlushElementTree", 0, move |_arguments| {
        tree.borrow_mut().flush();
        on_flush();
        Ok(HostValue::Undefined)
    })?;

    Ok(handle)
}

/// The tag-specific constructors, which differ only in the tag they name.
/// `ElementTree` still spells each one out — the tag vocabulary is its
/// property, not this layer's — so the table holds the method, not a name.
type Constructor = fn(&mut ElementTree, ElementId) -> ElementId;
const CONSTRUCTORS: &[(&str, Constructor)] = &[
    ("__CreateView", ElementTree::create_view),
    ("__CreateText", ElementTree::create_text),
    ("__CreateImage", ElementTree::create_image),
    ("__CreateScrollView", ElementTree::create_scroll_view),
    (
        "__CreateWrapperElement",
        ElementTree::create_wrapper_element,
    ),
    ("__CreateFrame", ElementTree::create_frame),
];

/// The read-only navigation members, which differ only in the step they take.
type Navigator = fn(&ElementTree, ElementId) -> ElementId;
const NAVIGATORS: &[(&str, Navigator)] = &[
    ("__GetParent", ElementTree::parent_element),
    ("__FirstElement", ElementTree::first_element),
    ("__LastElement", ElementTree::last_element),
    ("__NextElement", ElementTree::next_element),
];

/// The `__Create*` members. Each returns the new element's unique id.
fn install_creation(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // `__CreatePage(componentID, componentCSSID)` — idempotent; returns the
    // page's unique id.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreatePage", 2, move |arguments| {
        let component_id = string_argument("__CreatePage", arguments, 0)?;
        let component_css_id = i32_argument("__CreatePage", arguments, 1)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_page(component_id, component_css_id);
        Ok(unique_id_value(id))
    })?;

    // `__CreateElement(tagName, parentComponentUniqueID)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreateElement", 2, move |arguments| {
        let tag = string_argument("__CreateElement", arguments, 0)?;
        let parent_component = component_argument(arguments, 1);
        let id = tree
            .borrow_mut()
            .tree()
            .create_element(tag, parent_component);
        Ok(unique_id_value(id))
    })?;

    for (name, construct) in CONSTRUCTORS {
        let tree = Rc::clone(handle);
        let construct = *construct;
        realm.define_global_function(name, 1, move |arguments| {
            let parent_component = component_argument(arguments, 0);
            let id = construct(tree.borrow_mut().tree(), parent_component);
            Ok(unique_id_value(id))
        })?;
    }

    // `__CreateRawText(text)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreateRawText", 1, move |arguments| {
        let text = string_argument("__CreateRawText", arguments, 0)?;
        let id = tree.borrow_mut().tree().create_raw_text(text);
        Ok(unique_id_value(id))
    })?;

    Ok(())
}

/// The members that move elements around the tree.
fn install_structure(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // `__AppendElement(parent, child)` — returns the child unique id.
    let tree = Rc::clone(handle);
    realm.define_global_function("__AppendElement", 2, move |arguments| {
        let parent = element_argument("__AppendElement", arguments, 0)?;
        let child = element_argument("__AppendElement", arguments, 1)?;
        let appended = tree
            .borrow_mut()
            .tree()
            .append_element(parent, child)
            .map_err(papi_error)?;
        Ok(unique_id_value(appended))
    })?;

    // `__InsertElementBefore(parent, child, reference)` — a nullish reference
    // appends, exactly as `insertBefore(child, null)` does.
    let tree = Rc::clone(handle);
    realm.define_global_function("__InsertElementBefore", 3, move |arguments| {
        let parent = element_argument("__InsertElementBefore", arguments, 0)?;
        let child = element_argument("__InsertElementBefore", arguments, 1)?;
        let reference = optional_element_argument("__InsertElementBefore", arguments, 2)?;
        let inserted = tree
            .borrow_mut()
            .tree()
            .insert_element_before(parent, child, reference)
            .map_err(papi_error)?;
        Ok(unique_id_value(inserted))
    })?;

    // `__RemoveElement(parent, child)` — detaches; the child stays alive.
    let tree = Rc::clone(handle);
    realm.define_global_function("__RemoveElement", 2, move |arguments| {
        let parent = element_argument("__RemoveElement", arguments, 0)?;
        let child = element_argument("__RemoveElement", arguments, 1)?;
        let removed = tree
            .borrow_mut()
            .tree()
            .remove_element(parent, child)
            .map_err(papi_error)?;
        Ok(unique_id_value(removed))
    })?;

    // `__ReplaceElement(newElement, oldElement)` — new element first.
    let tree = Rc::clone(handle);
    realm.define_global_function("__ReplaceElement", 2, move |arguments| {
        let new_element = element_argument("__ReplaceElement", arguments, 0)?;
        let old_element = element_argument("__ReplaceElement", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .replace_element(new_element, old_element)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__SwapElement(a, b)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SwapElement", 2, move |arguments| {
        let first = element_argument("__SwapElement", arguments, 0)?;
        let second = element_argument("__SwapElement", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .swap_element(first, second)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// `__DropElement(element)` — the disposal notification.
///
/// This is **not** a web-core PAPI member, and its absence there is not an
/// omission: web-core hands the script real `HTMLElement` objects and so
/// reclaims an element's engine-side storage from a `WeakRef` sweep once the
/// script drops its last reference (`MainThreadWasmContext::gc`). Handles
/// cross this boundary as `u32` numbers, so the host has nothing to hold
/// weakly and nothing to observe — the script must say when a handle has
/// become garbage. `__DropElement` is that call, and the realm-side
/// `FinalizationRegistry` a `__Create*` member registers each new handle with
/// is what drives it.
///
/// It is the counterpart of `__RemoveElement`, not a variant of it:
/// `__RemoveElement` detaches an element that is still referenced and still
/// re-insertable, while this retires the handle and takes the storage.
fn install_disposal(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // A repeated drop stays a no-op: a finalizer runs at a time the collector
    // chooses, so the same handle arriving twice — or arriving after the
    // subtree was already retired with an ancestor — is ordinary rather than
    // an error. Dropping the permanent page element still is one.
    let tree = Rc::clone(handle);
    realm.define_global_function("__DropElement", 1, move |arguments| {
        let id = element_argument("__DropElement", arguments, 0)?;
        match tree.borrow_mut().tree().drop_element(id) {
            Ok(()) | Err(PapiError::UnknownElement(_)) => {}
            Err(error) => return Err(papi_error(error)),
        }
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// The read-only members: navigation, identity, and the tag.
fn install_queries(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    for (name, navigate) in NAVIGATORS {
        let tree = Rc::clone(handle);
        let navigate = *navigate;
        realm.define_global_function(name, 1, move |arguments| {
            let id = element_argument(name, arguments, 0)?;
            let related = navigate(tree.borrow_mut().tree(), id);
            // web-core answers `null`, not a sentinel, for "no such relative".
            Ok(if related == NO_ELEMENT {
                HostValue::Null
            } else {
                unique_id_value(related)
            })
        })?;
    }

    // `__ElementIsEqual(left, right)` — `left === right` in web-core
    // (`pureElementPAPIs.ts:49-52`) and pointer identity natively
    // (`renderer_functions.cc:3944`), which is handle equality here. `null` is
    // a legal operand on either side, and two absent operands really are
    // equal, so the comparison is made after both collapse to the sentinel
    // rather than being short-circuited on it.
    realm.define_global_function("__ElementIsEqual", 2, move |arguments| {
        let left = optional_element_argument("__ElementIsEqual", arguments, 0)?;
        let right = optional_element_argument("__ElementIsEqual", arguments, 1)?;
        Ok(HostValue::Boolean(left == right))
    })?;

    // `__GetElementUniqueID(element)` — `-1` for anything that is not a live
    // element, and never a throw.
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetElementUniqueID", 1, move |arguments| {
        let id = match argument(arguments, 0) {
            HostValue::Number(value) if value.is_finite() && *value >= 0.0 => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a value outside the u32 range names no element, and -1 is the answer either way"
                )]
                let id = *value as u32;
                tree.borrow_mut().tree().unique_id(id)
            }
            _ => -1,
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "a unique id is a u32 and -1; both are exact in f64"
        )]
        Ok(HostValue::Number(id as f64))
    })?;

    // `__GetPageElement()` — `undefined` before `__CreatePage`, matching
    // web-core's uninitialized `page` binding.
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetPageElement", 0, move |_arguments| {
        Ok(tree
            .borrow_mut()
            .tree()
            .page()
            .map_or(HostValue::Undefined, unique_id_value))
    })?;

    // `__GetTag(element)` — the Lynx tag name.
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetTag", 1, move |arguments| {
        let id = element_argument("__GetTag", arguments, 0)?;
        let mut handle = tree.borrow_mut();
        let tag = handle
            .tree()
            .tag(id)
            .ok_or_else(|| papi_error(PapiError::UnknownElement(id)))?;
        Ok(HostValue::String(tag.to_owned()))
    })?;

    // `__ChildCount`/`__ChildAt` back the prelude's `__GetChildren`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__ChildCount", 1, move |arguments| {
        let id = element_argument("__GetChildren", arguments, 0)?;
        let mut handle = tree.borrow_mut();
        let tree = handle.tree();
        let mut count = 0_u32;
        let mut child = tree.first_element(id);
        while child != NO_ELEMENT {
            count += 1;
            child = tree.next_element(child);
        }
        Ok(HostValue::Number(f64::from(count)))
    })?;
    let tree = Rc::clone(handle);
    realm.define_global_function("__ChildAt", 2, move |arguments| {
        let id = element_argument("__GetChildren", arguments, 0)?;
        let index = u32_argument("__GetChildren", arguments, 1)?;
        let mut handle = tree.borrow_mut();
        let tree = handle.tree();
        let mut child = tree.first_element(id);
        for _ in 0..index {
            child = tree.next_element(child);
        }
        Ok(if child == NO_ELEMENT {
            HostValue::Null
        } else {
            unique_id_value(child)
        })
    })?;

    Ok(())
}

/// Attributes and the element id.
fn install_properties(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // `__SetAttribute(element, key, value)` — a nullish value removes.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetAttribute", 3, move |arguments| {
        let id = element_argument("__SetAttribute", arguments, 0)?;
        let key = string_argument("__SetAttribute", arguments, 1)?;
        let value = papi_value("__SetAttribute", argument(arguments, 2))?;
        tree.borrow_mut()
            .tree()
            .set_attribute(id, key, &value)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__GetAttributeByName(element, name)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetAttributeByName", 2, move |arguments| {
        let id = element_argument("__GetAttributeByName", arguments, 0)?;
        let name = string_argument("__GetAttributeByName", arguments, 1)?;
        let mut handle = tree.borrow_mut();
        Ok(handle
            .tree()
            .attribute(id, name)
            .map_or(HostValue::Null, |value| HostValue::String(value.to_owned())))
    })?;

    // `__SetID(element, id)` / `__GetID(element)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetID", 2, move |arguments| {
        let id = element_argument("__SetID", arguments, 0)?;
        let value = optional_text("__SetID", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .set_id(id, value.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetID", 1, move |arguments| {
        let id = element_argument("__GetID", arguments, 0)?;
        let mut handle = tree.borrow_mut();
        Ok(handle
            .tree()
            .id_attribute(id)
            .map_or(HostValue::Null, |value| HostValue::String(value.to_owned())))
    })?;

    Ok(())
}

/// Classes and inline styles.
fn install_styling(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // `__AddClass(element, className)` / `__SetClasses(element, classNames)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__AddClass", 2, move |arguments| {
        let id = element_argument("__AddClass", arguments, 0)?;
        let class = string_argument("__AddClass", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .add_class(id, class)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetClasses", 2, move |arguments| {
        let id = element_argument("__SetClasses", arguments, 0)?;
        let value = optional_text("__SetClasses", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .set_classes(id, value.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;
    // `__GetClassText` backs the prelude's `__GetClasses`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetClassText", 1, move |arguments| {
        let id = element_argument("__GetClasses", arguments, 0)?;
        let mut handle = tree.borrow_mut();
        let classes: Vec<&str> = handle.tree().classes(id).collect();
        Ok(HostValue::String(classes.join(" ")))
    })?;

    // `__SetInlineStyleText` backs the prelude's `__SetInlineStyles`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetInlineStyleText", 2, move |arguments| {
        let id = element_argument("__SetInlineStyles", arguments, 0)?;
        let value = optional_text("__SetInlineStyles", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .set_inline_styles(id, value.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__AddInlineStyle(element, key, value)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__AddInlineStyle", 3, move |arguments| {
        let id = element_argument("__AddInlineStyle", arguments, 0)?;
        let property = match argument(arguments, 1) {
            HostValue::String(property) => property.clone(),
            &HostValue::Number(key) => return Err(papi_error(PapiError::NumericStyleKey(key))),
            other => {
                return Err(HostFunctionError::new(format!(
                    "__AddInlineStyle expects a CSS property name for argument 1, got {other:?}"
                )));
            }
        };
        let value = optional_text("__AddInlineStyle", arguments, 2)?;
        tree.borrow_mut()
            .tree()
            .add_inline_style(id, &property, value.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// The dataset members.
fn install_dataset(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // `__AddDatasetEntry`/`__ClearDataset` back the prelude's `__AddDataset`
    // and `__SetDataset`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__AddDatasetEntry", 3, move |arguments| {
        let id = element_argument("__AddDataset", arguments, 0)?;
        let key = string_argument("__AddDataset", arguments, 1)?;
        let value = papi_value("__AddDataset", argument(arguments, 2))?;
        tree.borrow_mut()
            .tree()
            .add_dataset(id, key, value)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;
    let tree = Rc::clone(handle);
    realm.define_global_function("__ClearDataset", 1, move |arguments| {
        let id = element_argument("__SetDataset", arguments, 0)?;
        tree.borrow_mut()
            .tree()
            .clear_dataset(id)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__GetDataByKey(element, key)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetDataByKey", 2, move |arguments| {
        let id = element_argument("__GetDataByKey", arguments, 0)?;
        let key = string_argument("__GetDataByKey", arguments, 1)?;
        let mut handle = tree.borrow_mut();
        Ok(handle
            .tree()
            .data_by_key(id, key)
            .map_or(HostValue::Undefined, host_value))
    })?;

    Ok(())
}

/// The CSS-scope and component-identity members.
fn install_scope(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // `__SetCSSIdOfElement` backs the prelude's `__SetCSSId`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetCSSIdOfElement", 3, move |arguments| {
        let id = element_argument("__SetCSSId", arguments, 0)?;
        let css_id = i32_argument("__SetCSSId", arguments, 1)?;
        let entry_name = string_argument("__SetCSSId", arguments, 2)?;
        let entry_name = (!entry_name.is_empty()).then_some(entry_name);
        tree.borrow_mut()
            .tree()
            .set_css_id(id, css_id, entry_name)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__UpdateComponentID(element, componentID)` / `__GetComponentID`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__UpdateComponentID", 2, move |arguments| {
        let id = element_argument("__UpdateComponentID", arguments, 0)?;
        let component_id = string_argument("__UpdateComponentID", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .update_component_id(id, component_id)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetComponentID", 1, move |arguments| {
        let id = element_argument("__GetComponentID", arguments, 0)?;
        let mut handle = tree.borrow_mut();
        Ok(handle
            .tree()
            .component_id(id)
            .map_or(HostValue::Null, |value| HostValue::String(value.to_owned())))
    })?;

    Ok(())
}

/// A unique id crossing the primitives-only native host boundary.
fn unique_id_value(id: ElementId) -> HostValue {
    HostValue::Number(f64::from(id))
}

fn papi_error(error: impl fmt::Display) -> HostFunctionError {
    HostFunctionError::new(error.to_string())
}

fn argument(arguments: &[HostValue], index: usize) -> &HostValue {
    arguments.get(index).unwrap_or(&HostValue::Undefined)
}

/// Reads an argument the PAPI carries as an opaque JavaScript primitive —
/// an attribute value, a dataset value, an id, a class list.
///
/// `HostValue` is `#[non_exhaustive]`, and a kind the bridge learns to carry
/// later would need a deliberate decision here rather than a silent coercion,
/// so an unrecognized one is an error at the call site.
fn papi_value(function: &str, value: &HostValue) -> Result<PapiValue, HostFunctionError> {
    Ok(match value {
        HostValue::Undefined => PapiValue::Undefined,
        HostValue::Null => PapiValue::Null,
        HostValue::Boolean(value) => PapiValue::Boolean(*value),
        HostValue::Number(value) => PapiValue::Number(*value),
        HostValue::String(value) => PapiValue::String(value.clone()),
        other => {
            return Err(HostFunctionError::new(format!(
                "{function} cannot carry a {other:?} value"
            )));
        }
    })
}

/// A nullish value clears; anything else is written through ECMAScript string
/// coercion. Shared by every member whose argument is "a string or nothing".
fn optional_text(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<Option<String>, HostFunctionError> {
    let value = papi_value(function, argument(arguments, index))?;
    Ok((!value.is_nullish()).then(|| value.to_string()))
}

/// The inverse, for the members that read a stored value back out.
fn host_value(value: &PapiValue) -> HostValue {
    match value {
        PapiValue::Undefined => HostValue::Undefined,
        PapiValue::Null => HostValue::Null,
        PapiValue::Boolean(value) => HostValue::Boolean(*value),
        PapiValue::Number(value) => HostValue::Number(*value),
        PapiValue::String(value) => HostValue::String(value.clone()),
    }
}

/// Reads an argument that must be an unsigned 32-bit integer, the way element
/// ids are represented by the runtime.
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
        // web-core defaults a missing/nullish componentCSSID to 0.
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

/// Borrows a string argument. The slice outlives the call, so there is no
/// reason to copy here — the tree makes the one copy it needs.
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

/// Reads an element-handle argument. `0` is the "no element" sentinel and is
/// never a valid handle for a PAPI call that must act on an element.
fn element_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<ElementId, HostFunctionError> {
    let id = u32_argument(function, arguments, index)?;
    (id != NO_ELEMENT).then_some(id).ok_or_else(|| {
        HostFunctionError::new(format!(
            "{function} expects an element handle for argument {index}, got the null handle 0"
        ))
    })
}

/// Reads a handle argument that is allowed to be absent — an
/// `__InsertElementBefore` reference, an `__ElementIsEqual` operand.
/// Nullish and the `0` sentinel all mean [`NO_ELEMENT`].
fn optional_element_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<ElementId, HostFunctionError> {
    match argument(arguments, index) {
        HostValue::Undefined | HostValue::Null => Ok(NO_ELEMENT),
        _ => u32_argument(function, arguments, index),
    }
}

/// Reads a `parentComponentUniqueID` argument.
///
/// Deliberately total: web-core looks the handle up to seed the new element's
/// CSS scope and falls back to "no scope" on a miss, so a stale or nonsensical
/// value produces an unscoped element rather than an exception. Rejecting it
/// would turn `ReactLynx`'s page-teardown race — where `__pageId` has been reset
/// to `0` — into a hard failure.
fn component_argument(arguments: &[HostValue], index: usize) -> ElementId {
    match *argument(arguments, index) {
        HostValue::Number(value) if value.is_finite() && value >= 0.0 => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an out-of-range value names no component, which is the same as no scope"
            )]
            let id = value as u32;
            id
        }
        _ => NO_ELEMENT,
    }
}
