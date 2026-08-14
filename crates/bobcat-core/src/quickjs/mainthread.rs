//! Main-thread (MTS) script execution and the native `bobcat` realm object.
//!
//! This is the crate `AGENTS.md` designates for "Lynx host globals". The
//! generic `QuickJS` bridge below stays Lynx-unaware; the Element PAPI
//! surface itself lives in JavaScript (`packages/bobcat-element`, embedded
//! here with `include_str!`). What is assembled here is the realm a
//! `.web.bundle`'s `lepusCode.root` runs in:
//!
//! 1. A global `bobcat` object whose members are Rust host functions speaking DOM vocabulary over
//!    numeric `NodeId`s — `createPage`, `createElement`, `setAttribute`, `insertBefore`,
//!    `removeElement`, `replaceElement`, `dropElement`, `flushElementTree` — each a direct call
//!    into [`dom::Document`] through the document hand-off.
//! 2. The Element PAPI runtime script, evaluated before any bundle code. It assigns the
//!    `__Create*`/`__*Element`/`__FlushElementTree` globals over `bobcat`, owns tag vocabulary, and
//!    manages handle lifecycle with a symbol-keyed node id and a `FinalizationRegistry`.
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
//! [`MainThreadRuntime`] follows that shape exactly — a `QuickJS` realm
//! standing in for the iframe, the `bobcat` object and PAPI runtime installed
//! before evaluation, the same wrapper, the same post-evaluation sequence.
//!
//! # The realm takes the tree for a batch, and returns it at the flush
//!
//! The [`LynxDocument`](crate::tree::LynxDocument) changes hands through a
//! [`SharedTree`] slot. A batch's first `bobcat` call takes the tree out;
//! every call after that is a plain `&mut` mutation with no
//! synchronization — the tree's own validation is the single source of
//! every structural `PapiError`, throwing at the call site.
//! `bobcat.flushElementTree` is the commit boundary: it runs the style +
//! layout commit on the taken tree, puts it back in the slot, and then
//! notifies the presenter through the injected callback. While the tree is
//! away the presenter works from its retained frame, so a half-applied batch
//! is unobservable. A script that opens a batch and returns without flushing
//! gets the tree put back uncommitted at the end of the evaluation — the
//! presenter's `has_uncommitted_mutations` gate keeps that state off the
//! screen.
//!
//! # Handle collection is delivered at realm entries
//!
//! When `QuickJS` collects an element handle, the PAPI runtime's
//! `FinalizationRegistry` cleanup callback (a pending job, executed at the
//! job checkpoint that follows every evaluation) queues the node id.
//! Queued drops are applied by the runtime's deliver hook, which
//! [`MainThreadRuntime`] calls before each realm entry and inside
//! [`MainThreadRuntime::collect_garbage`]. `FinalizationRegistry` sweeps run
//! only during an actual collection (allocation pressure or an explicit
//! [`collect_garbage`](MainThreadRuntime::collect_garbage)), never during
//! realm teardown — so dropping the runtime preserves the last committed
//! tree.
//!
//! # Recorded limits
//!
//! - **The PAPI surface is the runtime script's table** — every `ReactLynx` Snapshot constructor
//!   except `__CreateFrame`, the four tree mutations, and `__FlushElementTree`. A bundle that
//!   reaches for another member gets a `ReferenceError` naming the missing global, which is the
//!   intended failure: a silently wrong render would be worse.
//! - **Nothing validates script input.** A stale or fabricated node id panics inside `dom`; the
//!   host boundary converts the unwind into a JavaScript exception ("the host function panicked").
//! - **The non-element main-thread globals are absent** (`lynx`, `SystemInfo`, `__globalProps`,
//!   `_ReportError`, `__OnLifecycleEvent`, `__LoadLepusChunk`, `_I18nResourceTranslation`,
//!   `_AddEventListener`, `__QueryComponent`).
//! - **No background thread.** web-core starts the BTS worker between `processData` and
//!   `renderPage`; there is no second realm here, so `/app-service.js` is never loaded and
//!   `callLepusMethod` has no caller.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use quickjs_rust_bridge::{self as quickjs, HostFunctionError, HostValue};

use super::{QuickJsCallable, QuickJsInitializationError, QuickJsScriptEngine};
use crate::script::{ScriptEngine as _, ScriptError, ScriptValue};
use crate::tree::LynxDocument;

const MAIN_THREAD_SOURCE_NAME: &str = "main-thread.js";
const BOOT_SOURCE_NAME: &str = "<lynx boot>";
const BOBCAT_NAMESPACE_SOURCE_NAME: &str = "<bobcat namespace>";
const ELEMENT_PAPI_SOURCE_NAME: &str = "element-papi.js";
const DELIVER_HOOK_SOURCE_NAME: &str = "<bobcat deliver hook>";

/// The Element PAPI runtime, authored in `packages/bobcat-element` where its
/// Rstest suite runs the same bytes this realm evaluates.
const ELEMENT_PAPI_SOURCE: &str =
    include_str!("../../../../packages/bobcat-element/src/element-papi.js");

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
    taken: Option<LynxDocument>,
}

impl TreeHandle {
    fn tree(&mut self) -> &mut LynxDocument {
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
        tree.layout();
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

/// One `QuickJS` realm carrying the `bobcat` object and Element PAPI runtime
/// over the tree hand-off slot.
pub struct MainThreadRuntime {
    engine: QuickJsScriptEngine,
    tree: Rc<RefCell<TreeHandle>>,
    /// The PAPI runtime's deliver hook: applies unique ids queued by
    /// collected handles. Called before each realm entry.
    deliver_drops: QuickJsCallable,
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
        // Teardown must not look like application GC: queued handle drops die
        // with the realm, and `Engine::run_script` retains the last committed
        // tree after its short-lived bootstrap realm goes away.
        self.tree.borrow_mut().release();
    }
}

impl MainThreadRuntime {
    /// Creates a realm whose `bobcat` object takes the tree from `elements`
    /// per batch and mutates it directly, and installs it plus the Element
    /// PAPI runtime before any script has run.
    pub fn new(
        elements: SharedTree,
        on_flush: impl Fn() + 'static,
    ) -> Result<Self, QuickJsInitializationError> {
        let mut engine = QuickJsScriptEngine::new()?;
        let (tree, deliver_drops) = install_bobcat(&mut engine, elements, on_flush)?;
        Ok(Self {
            engine,
            tree,
            deliver_drops,
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

    pub fn collect_garbage(&mut self) -> Result<(), MainThreadError> {
        self.engine.realm.run_gc();
        let checkpoint = self
            .engine
            .checkpoint(crate::script::ScriptErrorPhase::Call)
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine("collecting garbage", &error));
        let result = checkpoint.and_then(|()| self.deliver_pending_drops());
        self.tree.borrow_mut().release();
        result
    }

    fn evaluate(&mut self, source: &str, name: &str, phase: &str) -> Result<(), MainThreadError> {
        let delivered = self.deliver_pending_drops();
        let result = delivered.and_then(|()| {
            self.engine
                .evaluate_raw(quickjs::EvalSource {
                    name: Some(name),
                    ..quickjs::EvalSource::new(source)
                })
                .map(|_| ())
                .map_err(|error| MainThreadError::from_engine(phase, &error))
        });
        self.tree.borrow_mut().release();
        result
    }

    fn deliver_pending_drops(&mut self) -> Result<(), MainThreadError> {
        let deliver = self.deliver_drops.clone();
        self.engine
            .call(&deliver, &ScriptValue::Undefined, &[])
            .map(|_| ())
            .map_err(|error| {
                MainThreadError::from_engine("delivering pending element drops", &error)
            })
    }
}

/// Installs the `bobcat` object and the Element PAPI runtime, returning the
/// tree hand-off handle and the runtime's deliver hook.
fn install_bobcat(
    engine: &mut QuickJsScriptEngine,
    elements: SharedTree,
    on_flush: impl Fn() + 'static,
) -> Result<(Rc<RefCell<TreeHandle>>, QuickJsCallable), QuickJsInitializationError> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        slot: elements,
        taken: None,
    }));

    install_bobcat_object(&mut engine.realm, &handle, on_flush)
        .map_err(QuickJsInitializationError::from_quickjs)?;

    engine
        .evaluate_raw(quickjs::EvalSource {
            name: Some(ELEMENT_PAPI_SOURCE_NAME),
            ..quickjs::EvalSource::new(ELEMENT_PAPI_SOURCE)
        })
        .map_err(QuickJsInitializationError::from_script)?;

    let deliver = engine
        .evaluate_raw(quickjs::EvalSource {
            name: Some(DELIVER_HOOK_SOURCE_NAME),
            ..quickjs::EvalSource::new("bobcat.deliverPendingElementDrops")
        })
        .map_err(QuickJsInitializationError::from_script)?;
    if deliver.kind() != quickjs::ValueKind::Function {
        return Err(QuickJsInitializationError::from_message(
            "the Element PAPI runtime did not install its deliver hook",
        ));
    }

    Ok((handle, QuickJsCallable(deliver)))
}

/// Builds the global `bobcat` namespace object: DOM-vocabulary tree
/// operations over numeric unique ids, one Rust host function per member.
fn install_bobcat_object(
    realm: &mut quickjs::Realm,
    handle: &Rc<RefCell<TreeHandle>>,
    on_flush: impl Fn() + 'static,
) -> Result<(), quickjs::Error> {
    let namespace = realm.evaluate(
        quickjs::EvalSource {
            name: Some(BOBCAT_NAMESPACE_SOURCE_NAME),
            ..quickjs::EvalSource::new("({})")
        },
        quickjs::EvalOptions::default(),
    )?;

    let tree = Rc::clone(handle);
    let member = realm.function("createPage", 0, move |_arguments| {
        let node = tree.borrow_mut().tree().document_element().id();
        Ok(node_id_value(node))
    })?;
    realm.set_property(&namespace, "createPage", &member)?;

    let tree = Rc::clone(handle);
    let member = realm.function("createElement", 1, move |arguments| {
        let tag = string_argument("bobcat.createElement", arguments, 0)?;
        let node = tree.borrow_mut().tree().create_element(tag, ());
        Ok(node_id_value(node))
    })?;
    realm.set_property(&namespace, "createElement", &member)?;

    let tree = Rc::clone(handle);
    let member = realm.function("setAttribute", 3, move |arguments| {
        let node = node_id_argument("bobcat.setAttribute", arguments, 0)?;
        let name = string_argument("bobcat.setAttribute", arguments, 1)?;
        let value = string_argument("bobcat.setAttribute", arguments, 2)?;
        tree.borrow_mut().tree().set_attribute(node, name, value);
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&namespace, "setAttribute", &member)?;

    let tree = Rc::clone(handle);
    let member = realm.function("insertBefore", 3, move |arguments| {
        let parent = node_id_argument("bobcat.insertBefore", arguments, 0)?;
        let child = node_id_argument("bobcat.insertBefore", arguments, 1)?;
        let reference = optional_node_id_argument("bobcat.insertBefore", arguments, 2)?;
        tree.borrow_mut()
            .tree()
            .insert_before(parent, child, reference);
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&namespace, "insertBefore", &member)?;

    let tree = Rc::clone(handle);
    let member = realm.function("removeElement", 1, move |arguments| {
        let child = node_id_argument("bobcat.removeElement", arguments, 0)?;
        tree.borrow_mut().tree().remove_element(child);
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&namespace, "removeElement", &member)?;

    let tree = Rc::clone(handle);
    let member = realm.function("replaceElement", 2, move |arguments| {
        let new_element = node_id_argument("bobcat.replaceElement", arguments, 0)?;
        let old_element = node_id_argument("bobcat.replaceElement", arguments, 1)?;
        let mut handle = tree.borrow_mut();
        let document = handle.tree();
        // ChildNode.replaceWith over a detached old element is a no-op.
        if let Some(parent) = document.get(old_element).and_then(dom::Node::parent_id) {
            document.insert_before(parent, new_element, Some(old_element));
            document.remove_element(old_element);
        }
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&namespace, "replaceElement", &member)?;

    let tree = Rc::clone(handle);
    let member = realm.function("dropElement", 1, move |arguments| {
        let node = node_id_argument("bobcat.dropElement", arguments, 0)?;
        tree.borrow_mut().tree().drop_element(node);
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&namespace, "dropElement", &member)?;

    let tree = Rc::clone(handle);
    let member = realm.function("flushElementTree", 0, move |_arguments| {
        tree.borrow_mut().flush();
        on_flush();
        Ok(HostValue::Undefined)
    })?;
    realm.set_property(&namespace, "flushElementTree", &member)?;

    let global = realm.global_object()?;
    realm.set_property(&global, "bobcat", &namespace)?;
    Ok(())
}

fn argument(arguments: &[HostValue], index: usize) -> &HostValue {
    arguments.get(index).unwrap_or(&HostValue::Undefined)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "slab indices stay far below f64's exact-integer range"
)]
fn node_id_value(node: dom::NodeId) -> HostValue {
    HostValue::Number(node as f64)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "garbage input maps to a garbage index and crashes in dom"
)]
fn node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<dom::NodeId, HostFunctionError> {
    let HostValue::Number(value) = *argument(arguments, index) else {
        return Err(HostFunctionError::new(format!(
            "{function} expects a number for argument {index}"
        )));
    };
    Ok(value as dom::NodeId)
}

fn optional_node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<Option<dom::NodeId>, HostFunctionError> {
    match *argument(arguments, index) {
        HostValue::Undefined | HostValue::Null => Ok(None),
        _ => node_id_argument(function, arguments, index).map(Some),
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
