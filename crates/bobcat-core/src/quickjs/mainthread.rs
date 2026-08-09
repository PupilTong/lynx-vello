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
//! # The prelude: object-shaped arguments over a primitives-only boundary
//!
//! The `QuickJS` host-function boundary carries primitives only — an object,
//! array or function passed to one is rejected rather than lossily converted.
//! Several Element PAPI members take object-shaped arguments anyway, and two of
//! the main-thread globals *are* objects. A small JavaScript prelude, evaluated
//! in the same realm before the bundle, closes that gap: it wraps the host
//! functions that need an argument flattened (`__SetCSSId`'s element list,
//! `__SetInlineStyles`' property map, `_ReportError`'s `Error`,
//! `__AddEvent`'s worklet handler) and defines `lynx` and `SystemInfo` in
//! JavaScript. web-core installs its own globals as closures over the realm's
//! global object for the same reason; the bundle sees the same names either
//! way.
//!
//! # Recorded limits
//!
//! - **The installed Element PAPI is the first-screen set** (see `lynx-element`'s crate docs for
//!   the table). A bundle that reaches for anything else gets a `ReferenceError` naming the missing
//!   global, which is the intended failure: a silently wrong render would be worse.
//! - **Element handles cross as `u32` unique-id numbers.** `__DropElement` asks the recorder to
//!   retire the handle; this layer does not add an object wrapper or GC policy around those ids.
//! - **`lynx` carries `__initData` and `SystemInfo` only.** `performance`, `getJSContext`,
//!   `getNative`, `queueMicrotask`, `requireModule` and the timer family are absent; a card root
//!   guards every one of them, and inventing a no-op would claim behavior that does not exist.
//!   `SystemInfo.lynxSdkVersion` is web-core's own `"3.0"`, and `pixelWidth`/`pixelHeight` are this
//!   view's physical size — a native embedder has no separate screen to measure.
//! - **`__OnLifecycleEvent` discards its argument.** Its only consumer is the background thread.
//! - **The remaining non-element globals are absent** (`__globalProps`, `_SetSourceMapRelease`,
//!   `__LoadLepusChunk`, `_I18nResourceTranslation`, `_AddEventListener`, `__QueryComponent`).
//! - **No background thread.** web-core starts the BTS worker between `processData` and
//!   `renderPage`; there is no second realm here, so `/app-service.js` is never loaded and
//!   `callLepusMethod` has no caller.

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use lynx_element::{ElementId, ElementTree, PapiError, Viewport};
use quickjs_rust_bridge::{self as quickjs, HostFunctionError, HostValue};

use super::{QuickJsInitializationError, QuickJsScriptEngine};
use crate::script::ScriptError;

/// The source name `QuickJS` reports for the main-thread bundle.
const MAIN_THREAD_SOURCE_NAME: &str = "main-thread.js";
/// The source name for the boot sequence this module drives.
const BOOT_SOURCE_NAME: &str = "<lynx boot>";
/// The source name for the prelude installed ahead of the bundle.
const PRELUDE_SOURCE_NAME: &str = "<lynx prelude>";

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

/// The realm prelude: the main-thread globals that are not plain primitive
/// calls.
///
/// The host-function boundary carries primitives only — a JavaScript object,
/// array or function passed to one is rejected rather than lossily converted
/// (`quickjs_rust_bridge`'s `HostValue`). Several Element PAPI members take
/// object-shaped arguments anyway, and two main-thread globals *are* objects.
/// This prelude is where that gap closes: it runs in the same realm before the
/// bundle, wraps the host functions that need object arguments flattened, and
/// defines the object globals in JavaScript, which is what web-core does too
/// (it installs closures over `mtsRealm.globalWindow`).
///
/// Each wrapper captures the host function and replaces the global, so the
/// bundle sees exactly the PAPI names web-core exposes.
fn prelude_source(viewport: Viewport) -> String {
    // `systemInfoBase` plus the view metrics, matching web-core's
    // `createSystemInfo`. Recorded limit: `pixelWidth`/`pixelHeight` are this
    // view's physical size — a native embedder has no separate screen to
    // measure, where a browser reads `window.screen`.
    let pixel_ratio = viewport.device_pixel_ratio;
    let pixel_width = viewport.width * pixel_ratio;
    let pixel_height = viewport.height * pixel_ratio;
    format!(
        r#"(function () {{
  "use strict";
  var global = globalThis;

  var hostSetInlineStyles = global.__SetInlineStyles;
  global.__SetInlineStyles = function (element, value) {{
    if (value === null || value === undefined || value === "") {{
      return hostSetInlineStyles(element, null);
    }}
    if (typeof value === "string") {{
      return hostSetInlineStyles(element, value);
    }}
    var css = "";
    var keys = Object.keys(value);
    for (var i = 0; i < keys.length; i++) {{
      var declaration = value[keys[i]];
      if (declaration === null || declaration === undefined) continue;
      css += hyphenate(keys[i]) + ":" + declaration + ";";
    }}
    return hostSetInlineStyles(element, css);
  }};

  // Property names in the object form arrive in the JavaScript spelling
  // (`backgroundColor`); web-core hyphenates them the same way before writing
  // the style attribute.
  function hyphenate(name) {{
    var out = "";
    for (var i = 0; i < name.length; i++) {{
      var character = name[i];
      var lower = character.toLowerCase();
      if (character !== lower) out += "-";
      out += lower;
    }}
    return out;
  }}

  var hostSetCSSId = global.__SetCSSId;
  global.__SetCSSId = function (elements, cssId, entryName) {{
    var list = Array.isArray(elements) ? elements : [elements];
    for (var i = 0; i < list.length; i++) {{
      hostSetCSSId(list[i], cssId === null || cssId === undefined ? 0 : cssId);
    }}
  }};

  var hostAddEvent = global.__AddEvent;
  global.__AddEvent = function (element, eventType, eventName, handler) {{
    return hostAddEvent(
      element,
      eventType,
      eventName,
      typeof handler === "string" ? handler : null
    );
  }};

  var hostReportError = global._ReportError;
  global._ReportError = function (error, info) {{
    // QuickJS puts only the frames in `stack`, so the message has to be taken
    // from the error itself.
    var text = String(error);
    if (error && error.stack) text += "\n" + error.stack;
    hostReportError(
      text,
      info && info.errorCode !== undefined ? String(info.errorCode) : null
    );
  }};

  // The background thread is the only consumer of lifecycle events, and there
  // is none here, so this discards its argument rather than crossing the
  // boundary with it (recorded limit).
  global.__OnLifecycleEvent = function () {{}};

  global.SystemInfo = {{
    platform: "web",
    lynxSdkVersion: "3.0",
    pixelRatio: {pixel_ratio},
    pixelWidth: {pixel_width},
    pixelHeight: {pixel_height}
  }};
  global.lynx = {{ __initData: {{}}, SystemInfo: global.SystemInfo }};
}})()"#
    )
}

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

/// Errors the main-thread script reported through `_ReportError` rather than
/// throwing.
///
/// `ReactLynx`'s render wraps the whole first screen in a `try`/`catch` whose
/// handler calls `lynx.reportError` and then *removes the rendered children*,
/// so a reported error during the boot means the first screen did not render.
/// Collecting them lets that surface as a failure instead of an empty frame.
type ReportedErrors = Rc<RefCell<Vec<String>>>;

/// One `QuickJS` realm carrying the Lynx Element PAPI over the tree
/// hand-off slot.
pub struct MainThreadRuntime {
    engine: QuickJsScriptEngine,
    tree: Rc<RefCell<TreeHandle>>,
    reported_errors: ReportedErrors,
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
        viewport: Viewport,
        on_flush: impl Fn() + 'static,
    ) -> Result<Self, QuickJsInitializationError> {
        let mut engine = QuickJsScriptEngine::new()?;
        let reported_errors = ReportedErrors::default();
        let tree = install_element_papi(
            &mut engine.realm,
            elements,
            Rc::clone(&reported_errors),
            on_flush,
        )
        .map_err(QuickJsInitializationError::from_quickjs)?;
        let mut runtime = Self {
            engine,
            tree,
            reported_errors,
        };
        // The prelude closes over the host functions installed above, so it runs
        // after them and before anything else can observe the globals.
        runtime
            .evaluate(
                &prelude_source(viewport),
                PRELUDE_SOURCE_NAME,
                "installing the main-thread globals",
            )
            .map_err(|error| QuickJsInitializationError::from_message(error.to_string()))?;
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
    ///
    /// An error the script *reported* rather than threw fails this call too:
    /// `ReactLynx` catches a failing first render, reports it, and removes the
    /// children it had built, so the alternative is an empty frame and no
    /// explanation.
    pub fn render_page(&mut self) -> Result<(), MainThreadError> {
        self.evaluate(BOOT_SEQUENCE, BOOT_SOURCE_NAME, "rendering the page")?;
        let reported = std::mem::take(&mut *self.reported_errors.borrow_mut());
        if reported.is_empty() {
            Ok(())
        } else {
            Err(MainThreadError {
                message: format!("rendering the page: {}", reported.join("; ")),
                location: None,
            })
        }
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
    reported_errors: ReportedErrors,
    on_flush: impl Fn() + 'static,
) -> Result<Rc<RefCell<TreeHandle>>, quickjs::Error> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        slot: elements,
        taken: None,
    }));
    install_constructors(realm, &handle)?;
    install_property_setters(realm, &handle)?;
    install_tree_editors(realm, &handle, on_flush)?;

    // `_ReportError(error, info)` — the prelude has already reduced the `Error`
    // object to its message and stack text.
    realm.define_global_function("_ReportError", 2, move |arguments| {
        let message = string_argument("_ReportError", arguments, 0)?;
        reported_errors.borrow_mut().push(message.to_owned());
        Ok(HostValue::Undefined)
    })?;

    Ok(handle)
}

/// The `__Create*` members: every one returns a fresh unique id.
fn install_constructors(
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

    // `__CreateView(parentComponentUniqueID)` — returns the new view's unique
    // id. The argument is `0` when there is no parent component.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreateView", 1, move |arguments| {
        let parent_component = u32_argument("__CreateView", arguments, 0)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_view(parent_component)
            .map_err(papi_error)?;
        Ok(unique_id_value(id))
    })?;

    // `__CreateElement(tagName, parentComponentUniqueID)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreateElement", 2, move |arguments| {
        let tag = string_argument("__CreateElement", arguments, 0)?;
        let parent_component = u32_argument("__CreateElement", arguments, 1)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_element(tag, parent_component)
            .map_err(papi_error)?;
        Ok(unique_id_value(id))
    })?;

    // `__CreateText(parentComponentUniqueID)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreateText", 1, move |arguments| {
        let parent_component = u32_argument("__CreateText", arguments, 0)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_text(parent_component)
            .map_err(papi_error)?;
        Ok(unique_id_value(id))
    })?;

    // `__CreateImage(parentComponentUniqueID)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreateImage", 1, move |arguments| {
        let parent_component = u32_argument("__CreateImage", arguments, 0)?;
        let id = tree
            .borrow_mut()
            .tree()
            .create_image(parent_component)
            .map_err(papi_error)?;
        Ok(unique_id_value(id))
    })?;

    // `__CreateRawText(text)` — takes the text itself, not a parent component.
    let tree = Rc::clone(handle);
    realm.define_global_function("__CreateRawText", 1, move |arguments| {
        let text = string_argument("__CreateRawText", arguments, 0)?;
        let id = tree.borrow_mut().tree().create_raw_text(text);
        Ok(unique_id_value(id))
    })?;

    Ok(())
}

/// The members that write an element's own selector-visible state, plus the
/// handle read `__GetElementUniqueID`.
fn install_property_setters(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), quickjs::Error> {
    // `__GetElementUniqueID(element)` — the handle is the unique id, so this
    // is its liveness check.
    let tree = Rc::clone(handle);
    realm.define_global_function("__GetElementUniqueID", 1, move |arguments| {
        let id = element_argument("__GetElementUniqueID", arguments, 0)?;
        let unique_id = tree
            .borrow_mut()
            .tree()
            .element_unique_id(id)
            .map_err(papi_error)?;
        Ok(unique_id_value(unique_id))
    })?;

    // `__SetClasses(element, classNames)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetClasses", 2, move |arguments| {
        let id = element_argument("__SetClasses", arguments, 0)?;
        let classes = value_argument("__SetClasses", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .set_classes(id, classes.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__SetID(element, id)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetID", 2, move |arguments| {
        let id = element_argument("__SetID", arguments, 0)?;
        let value = value_argument("__SetID", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .set_id(id, value.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__SetAttribute(element, key, value)`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetAttribute", 3, move |arguments| {
        let id = element_argument("__SetAttribute", arguments, 0)?;
        let name = string_argument("__SetAttribute", arguments, 1)?;
        let value = value_argument("__SetAttribute", arguments, 2)?;
        tree.borrow_mut()
            .tree()
            .set_attribute(id, name, value.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__SetInlineStyles(element, value)` — the prelude has already flattened
    // the object form into a declaration block string.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetInlineStyles", 2, move |arguments| {
        let id = element_argument("__SetInlineStyles", arguments, 0)?;
        let css = value_argument("__SetInlineStyles", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .set_inline_styles(id, css.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__SetCSSId(element, cssId)` — the prelude loops the element list over
    // this one-element form.
    let tree = Rc::clone(handle);
    realm.define_global_function("__SetCSSId", 2, move |arguments| {
        let id = element_argument("__SetCSSId", arguments, 0)?;
        let css_id = i32_argument("__SetCSSId", arguments, 1)?;
        tree.borrow_mut()
            .tree()
            .set_css_id(id, css_id)
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    // `__AddEvent(element, eventType, eventName, handler)` — the prelude has
    // already reduced a worklet handler object to `null`.
    let tree = Rc::clone(handle);
    realm.define_global_function("__AddEvent", 4, move |arguments| {
        let id = element_argument("__AddEvent", arguments, 0)?;
        let event_type = string_argument("__AddEvent", arguments, 1)?;
        let name = string_argument("__AddEvent", arguments, 2)?;
        let handler = value_argument("__AddEvent", arguments, 3)?;
        tree.borrow_mut()
            .tree()
            .add_event(id, event_type, name, handler.as_deref())
            .map_err(papi_error)?;
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// The members that change the tree's shape, plus the commit boundary.
fn install_tree_editors(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
    on_flush: impl Fn() + 'static,
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

    // `__DropElement(element)` — repeated drops stay no-ops (the handle is
    // already retired); dropping the permanent page element is a precise
    // error.
    let tree = Rc::clone(handle);
    realm.define_global_function("__DropElement", 1, move |arguments| {
        let id = element_argument("__DropElement", arguments, 0)?;
        match tree.borrow_mut().tree().drop_element(id) {
            Ok(()) | Err(PapiError::UnknownElement(_)) => {}
            Err(error) => return Err(papi_error(error)),
        }
        Ok(HostValue::Undefined)
    })?;

    // `__FlushElementTree()` — the single commit boundary: the style + layout
    // commit runs on the taken tree, the tree goes back in its slot, and the
    // presenter is notified. An empty batch still commits — a flush is a
    // style + layout pass even with nothing recorded. web-core ignores the
    // optional sub-tree and options arguments on the web target too.
    let tree = Rc::clone(handle);
    realm.define_global_function("__FlushElementTree", 0, move |_arguments| {
        tree.borrow_mut().flush();
        on_flush();
        Ok(HostValue::Undefined)
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

/// Reads an argument the way web-core's `String(value)` coercion does, keeping
/// the nullish case distinct because it removes rather than assigns.
///
/// Objects, arrays and functions never reach here: the host boundary rejects
/// them, and the prelude flattens the PAPI arguments that legitimately carry
/// one.
fn value_argument<'a>(
    function: &str,
    arguments: &'a [HostValue],
    index: usize,
) -> Result<Option<Cow<'a, str>>, HostFunctionError> {
    match argument(arguments, index) {
        HostValue::Undefined | HostValue::Null => Ok(None),
        HostValue::String(value) => Ok(Some(Cow::Borrowed(value.as_str()))),
        HostValue::Boolean(value) => Ok(Some(Cow::Owned(value.to_string()))),
        HostValue::Number(value) => Ok(Some(Cow::Owned(number_to_string(*value)))),
        _ => Err(HostFunctionError::new(format!(
            "{function} cannot convert argument {index} to a string"
        ))),
    }
}

/// `String(number)` for the values that reach an attribute or a declaration.
///
/// `f64::to_string` already prints an integral value without a fractional part,
/// which is what JavaScript does; the exponent thresholds differ, and no
/// framework-emitted attribute value reaches them.
fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value.is_sign_negative() {
            "-Infinity".to_owned()
        } else {
            "Infinity".to_owned()
        }
    } else {
        value.to_string()
    }
}

/// Borrows a string argument. The slice outlives the call, so there is no
/// reason to copy here — `create_page` makes the one copy it needs.
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
    (id != 0).then_some(id).ok_or_else(|| {
        HostFunctionError::new(format!(
            "{function} expects an element handle for argument {index}, got the null handle 0"
        ))
    })
}
