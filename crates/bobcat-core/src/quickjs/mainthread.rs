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
//! # The realm shares the tree under one lock
//!
//! The realm runs on the same thread that owns the Lynx main-thread work,
//! and the [`ElementTree`](lynx_element::ElementTree) is shared with the
//! presenting side behind one `Mutex`. Each PAPI global takes the lock for
//! the duration of one call and mutates the tree directly — the tree's own
//! validation is the single source of every `PapiError`. `__FlushElementTree`
//! is the commit boundary: it runs the style + layout commit under the lock,
//! clears the open batch, and then notifies the presenter through the
//! injected callback. Between a batch's first mutation and its flush the
//! tree reports uncommitted mutations, which is what keeps a presenter on
//! another thread from producing a frame out of a half-applied batch.
//!
//! # Recorded limits
//!
//! - **Five Element PAPI members are installed** — `__CreatePage`, `__CreateView`,
//!   `__AppendElement`, `__DropElement`, `__FlushElementTree` (see `lynx-element`'s crate docs). A
//!   bundle that reaches for anything else gets a `ReferenceError` naming the missing global, which
//!   is the intended failure: a silently wrong render would be worse.
//! - **Element handles cross as `u32` unique-id numbers.** `__DropElement` asks the recorder to
//!   retire the handle; this layer does not add an object wrapper or GC policy around those ids.
//! - **The non-element main-thread globals are absent** (`lynx`, `SystemInfo`, `__globalProps`,
//!   `_ReportError`, `__OnLifecycleEvent`, `__LoadLepusChunk`, `_I18nResourceTranslation`,
//!   `_AddEventListener`, `__QueryComponent`).
//! - **No background thread.** web-core starts the BTS worker between `processData` and
//!   `renderPage`; there is no second realm here, so `/app-service.js` is never loaded and
//!   `callLepusMethod` has no caller.

use std::fmt;
use std::sync::{Arc, Mutex};

use lynx_element::{ElementId, ElementTree, PapiError};
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

/// One `QuickJS` realm carrying the Lynx Element PAPI over the shared
/// element tree.
pub struct MainThreadRuntime {
    engine: QuickJsScriptEngine,
}

impl fmt::Debug for MainThreadRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadRuntime")
            .finish_non_exhaustive()
    }
}

impl MainThreadRuntime {
    /// Creates a realm whose Element PAPI mutates `elements` directly under
    /// its lock, and installs it before any script has run. `on_flush` runs
    /// after every committed `__FlushElementTree`, outside the lock — the
    /// seam a presenter uses to learn a committed frame is available.
    pub fn new(
        elements: Arc<Mutex<ElementTree>>,
        on_flush: impl Fn() + 'static,
    ) -> Result<Self, QuickJsInitializationError> {
        let mut engine = QuickJsScriptEngine::new()?;
        install_element_papi(&mut engine.realm, elements, on_flush)
            .map_err(QuickJsInitializationError::from_quickjs)?;
        Ok(Self { engine })
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
        self.engine
            .evaluate_raw(quickjs::EvalSource {
                name: Some(name),
                ..quickjs::EvalSource::new(source)
            })
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine(phase, &error))
    }
}

/// Installs the Element PAPI onto the realm's global object.
///
/// web-core does the equivalent with one `Object.assign` of a closure literal;
/// each closure here locks the same shared tree for the duration of one call.
/// Validation is the tree's own — a bad handle throws at the call site.
fn install_element_papi(
    realm: &mut quickjs::Realm,
    elements: Arc<Mutex<ElementTree>>,
    on_flush: impl Fn() + 'static,
) -> Result<(), quickjs::Error> {
    fn locked(elements: &Mutex<ElementTree>) -> std::sync::MutexGuard<'_, ElementTree> {
        elements
            .lock()
            .expect("the element tree lock is poisoned: the presenting side crashed")
    }

    // `__CreatePage(componentID, componentCSSID)` — idempotent; returns the
    // page's unique id.
    let tree = Arc::clone(&elements);
    realm.define_global_function("__CreatePage", 2, move |arguments| {
        let component_id = string_argument("__CreatePage", arguments, 0)?;
        let component_css_id = i32_argument("__CreatePage", arguments, 1)?;
        let id = locked(&tree).create_page(component_id, component_css_id);
        Ok(unique_id_value(id))
    })?;

    // `__CreateView(parentComponentUniqueID)` — returns the new view's unique
    // id. The argument is `0` when there is no parent component.
    let tree = Arc::clone(&elements);
    realm.define_global_function("__CreateView", 1, move |arguments| {
        let parent_component = u32_argument("__CreateView", arguments, 0)?;
        let id = locked(&tree)
            .create_view(parent_component)
            .map_err(papi_error)?;
        Ok(unique_id_value(id))
    })?;

    // `__AppendElement(parent, child)` — returns the child unique id.
    let tree = Arc::clone(&elements);
    realm.define_global_function("__AppendElement", 2, move |arguments| {
        let parent = element_argument("__AppendElement", arguments, 0)?;
        let child = element_argument("__AppendElement", arguments, 1)?;
        let appended = locked(&tree)
            .append_element(parent, child)
            .map_err(papi_error)?;
        Ok(unique_id_value(appended))
    })?;

    // `__DropElement(element)` — repeated drops stay no-ops (the handle is
    // already retired); dropping the permanent page element is a precise
    // error.
    let tree = Arc::clone(&elements);
    realm.define_global_function("__DropElement", 1, move |arguments| {
        let id = element_argument("__DropElement", arguments, 0)?;
        match locked(&tree).drop_element(id) {
            Ok(()) | Err(PapiError::UnknownElement(_)) => {}
            Err(error) => return Err(papi_error(error)),
        }
        Ok(HostValue::Undefined)
    })?;

    // `__FlushElementTree()` — the single commit boundary: the style + layout
    // commit runs under the lock, closing the batch the calls above opened,
    // and the presenter is notified once the lock is released. An empty batch
    // still commits — a flush is a style + layout pass even with nothing
    // recorded. web-core ignores the optional sub-tree and options arguments
    // on the web target too.
    realm.define_global_function("__FlushElementTree", 0, move |_arguments| {
        locked(&elements).flush_element_tree();
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
