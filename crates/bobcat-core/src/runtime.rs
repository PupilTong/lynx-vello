//! Engine-owned Lynx main-thread runtime over an injected JavaScript VM.

use std::cell::{RefCell, RefMut};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use crate::engine::SharedTree;
use crate::script::{HostValue, ScriptEngine, ScriptEngineFactory, ScriptError};
use crate::tree::LynxDocument;

const BOOT_SOURCE_NAME: &str = "<lynx boot>";
const ELEMENT_PAPI_SOURCE_NAME: &str = "element-papi.js";

const ELEMENT_PAPI_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/element-papi.js");

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

/// Why constructing or running the engine-owned main-thread runtime failed.
#[derive(Debug)]
pub(crate) struct MainThreadError {
    context: &'static str,
    source: ScriptError,
}

impl MainThreadError {
    fn from_engine(context: &'static str, source: ScriptError) -> Self {
        Self { context, source }
    }

    pub(crate) fn into_script_error(self) -> ScriptError {
        self.source
    }
}

impl fmt::Display for MainThreadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source.message)?;
        if let Some(location) = &self.source.location {
            let source = location.source.as_deref().unwrap_or("<unknown>");
            match (location.line, location.column) {
                (Some(line), Some(column)) => write!(formatter, " (at {source}:{line}:{column})")?,
                (Some(line), None) => write!(formatter, " (at {source}:{line})")?,
                _ => write!(formatter, " (at {source})")?,
            }
        }
        Ok(())
    }
}

impl std::error::Error for MainThreadError {}

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
        if self.taken.is_none() {
            self.taken = Some(self.slot.take());
        }
        self.taken
            .as_mut()
            .expect("the batch tree was just ensured")
            .layout();
        self.slot.put(
            self.taken
                .take()
                .expect("the laid-out batch tree is still owned"),
        );
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

/// The private main-thread runtime used by the engine pipeline.
pub(crate) struct MainThreadRuntime {
    engine: Box<dyn ScriptEngine>,
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
        self.tree.borrow_mut().release();
    }
}

impl MainThreadRuntime {
    pub(crate) fn new(
        factory: &dyn ScriptEngineFactory,
        elements: SharedTree,
        on_flush: impl Fn() + 'static,
    ) -> Result<Self, MainThreadError> {
        let mut engine = factory
            .create()
            .map_err(|error| MainThreadError::from_engine("creating the script VM", error))?;
        let tree = install_bobcat(engine.as_mut(), elements, on_flush)?;
        Ok(Self { engine, tree })
    }

    pub(crate) fn evaluate_main_thread_script(
        &mut self,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        let wrapped = format!("{WRAPPER_PREFIX}{source}{WRAPPER_SUFFIX}");
        self.evaluate(&wrapped, source_name, "evaluating the main-thread script")
    }

    pub(crate) fn render_page(&mut self) -> Result<(), MainThreadError> {
        self.evaluate(BOOT_SEQUENCE, BOOT_SOURCE_NAME, "rendering the page")
    }

    pub(crate) fn run_main_thread_script(
        &mut self,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        self.evaluate_main_thread_script(source, source_name)?;
        self.render_page()
    }

    #[allow(dead_code, reason = "used by the future runtime lifecycle surface")]
    pub(crate) fn collect_garbage(&mut self) -> Result<(), MainThreadError> {
        let result = self
            .engine
            .collect_garbage()
            .map_err(|error| MainThreadError::from_engine("collecting garbage", error));
        self.tree.borrow_mut().release();
        result
    }

    fn evaluate(
        &mut self,
        source: &str,
        name: &str,
        phase: &'static str,
    ) -> Result<(), MainThreadError> {
        let result = self
            .engine
            .execute_script(source, name)
            .map_err(|error| MainThreadError::from_engine(phase, error));
        self.tree.borrow_mut().release();
        result
    }
}

fn install_bobcat(
    engine: &mut dyn ScriptEngine,
    elements: SharedTree,
    on_flush: impl Fn() + 'static,
) -> Result<Rc<RefCell<TreeHandle>>, MainThreadError> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        slot: elements,
        taken: None,
    }));

    install_bobcat_object(engine, &handle, on_flush)?;
    engine
        .execute_script(ELEMENT_PAPI_SOURCE, ELEMENT_PAPI_SOURCE_NAME)
        .map_err(|error| MainThreadError::from_engine("installing the Element PAPI", error))?;

    Ok(handle)
}

fn install(
    engine: &mut dyn ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostValue]) -> Result<HostValue, String> + 'static,
) -> Result<(), MainThreadError> {
    engine
        .register_host_function("bobcat", name, arity, Box::new(callback))
        .map_err(|error| MainThreadError::from_engine("installing the bobcat namespace", error))
}

fn install_bobcat_object(
    engine: &mut dyn ScriptEngine,
    handle: &Rc<RefCell<TreeHandle>>,
    on_flush: impl Fn() + 'static,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, "createPage", 0, move |_arguments| {
        let mut tree = borrow_tree("bobcat.createPage", &tree)?;
        let node = tree.tree().document_element().id();
        Ok(node_id_value(node))
    })?;

    let tree = Rc::clone(handle);
    install(engine, "createElement", 1, move |arguments| {
        let tag = string_argument("bobcat.createElement", arguments, 0)?;
        let mut tree = borrow_tree("bobcat.createElement", &tree)?;
        let node = tree.tree().create_element(tag, ());
        Ok(node_id_value(node))
    })?;

    install_attribute_members(engine, handle)?;

    let tree = Rc::clone(handle);
    install(engine, "parentNode", 1, move |arguments| {
        let node = node_id_argument("bobcat.parentNode", arguments, 0)?;
        let mut tree = borrow_tree("bobcat.parentNode", &tree)?;
        let parent = tree.tree().get(node).and_then(dom::Node::parent_id);
        Ok(parent.map_or(HostValue::Null, node_id_value))
    })?;

    let tree = Rc::clone(handle);
    install(engine, "insertBefore", 3, move |arguments| {
        let parent = node_id_argument("bobcat.insertBefore", arguments, 0)?;
        let child = node_id_argument("bobcat.insertBefore", arguments, 1)?;
        let reference = optional_node_id_argument("bobcat.insertBefore", arguments, 2)?;
        let mut tree = borrow_tree("bobcat.insertBefore", &tree)?;
        let document = tree.tree();
        validate_insert(document, "bobcat.insertBefore", parent, child, reference)?;
        document.insert_before(parent, child, reference);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "removeElement", 1, move |arguments| {
        let child = node_id_argument("bobcat.removeElement", arguments, 0)?;
        let mut tree = borrow_tree("bobcat.removeElement", &tree)?;
        let document = tree.tree();
        validate_removable(document, "bobcat.removeElement", child)?;
        document.remove_element(child);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "replaceElement", 2, move |arguments| {
        let new_element = node_id_argument("bobcat.replaceElement", arguments, 0)?;
        let old_element = node_id_argument("bobcat.replaceElement", arguments, 1)?;
        let mut handle = borrow_tree("bobcat.replaceElement", &tree)?;
        let document = handle.tree();
        validate_removable(document, "bobcat.replaceElement", old_element)?;
        validate_live_element(document, "bobcat.replaceElement", new_element)?;
        if let Some(parent) = document.get(old_element).and_then(dom::Node::parent_id) {
            validate_insert(
                document,
                "bobcat.replaceElement",
                parent,
                new_element,
                Some(old_element),
            )?;
            document.insert_before(parent, new_element, Some(old_element));
            document.remove_element(old_element);
        }
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "swapElement", 2, move |arguments| {
        let a = node_id_argument("bobcat.swapElement", arguments, 0)?;
        let b = node_id_argument("bobcat.swapElement", arguments, 1)?;
        let mut tree = borrow_tree("bobcat.swapElement", &tree)?;
        let document = tree.tree();
        validate_swap(document, "bobcat.swapElement", a, b)?;
        document.swap_element(a, b);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "dropElement", 1, move |arguments| {
        let node = node_id_argument("bobcat.dropElement", arguments, 0)?;
        let mut tree = borrow_tree("bobcat.dropElement", &tree)?;
        let document = tree.tree();
        validate_removable(document, "bobcat.dropElement", node)?;
        document.drop_element(node);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "flushElementTree", 0, move |_arguments| {
        borrow_tree("bobcat.flushElementTree", &tree)?.flush();
        on_flush();
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// Installs the DOM-attribute and inline-style portion of the host namespace.
///
/// `id`, `class`, and `style` deliberately travel through the same narrow
/// string boundary as every other attribute. [`LynxDocument`] owns their
/// specialized DOM/style invalidation paths. `set_node_property` is the one
/// narrower CSSOM-like primitive: exactly one name/value pair per call, with
/// JavaScript retaining responsibility for fanning out a style record. Neither
/// the Element PAPI nor an injected VM receives a document handle.
fn install_attribute_members(
    engine: &mut dyn ScriptEngine,
    handle: &Rc<RefCell<TreeHandle>>,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, "setAttribute", 3, move |arguments| {
        let node = node_id_argument("bobcat.setAttribute", arguments, 0)?;
        let name = string_argument("bobcat.setAttribute", arguments, 1)?;
        let value = string_argument("bobcat.setAttribute", arguments, 2)?;
        let mut tree = borrow_tree("bobcat.setAttribute", &tree)?;
        let document = tree.tree();
        validate_live_element(document, "bobcat.setAttribute", node)?;
        document.set_attribute(node, name, value);
        Ok(HostValue::Undefined)
    })?;

    // Deliberately name-based: this PAPI receives record keys, custom
    // properties have no numeric id, and Stylo's internal PropertyId is not a
    // stable script ABI. A future numeric-key `__AddInlineStyle` can translate
    // its bundle id in JavaScript/the decoder-owned layer before reaching this
    // one primitive.
    let tree = Rc::clone(handle);
    install(engine, "set_node_property", 3, move |arguments| {
        let node = node_id_argument("bobcat.set_node_property", arguments, 0)?;
        let name = string_argument("bobcat.set_node_property", arguments, 1)?;
        let value = string_argument("bobcat.set_node_property", arguments, 2)?;
        let mut tree = borrow_tree("bobcat.set_node_property", &tree)?;
        let document = tree.tree();
        validate_live_element(document, "bobcat.set_node_property", node)?;
        document.set_inline_style_property(node, name, value);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "removeAttribute", 2, move |arguments| {
        let node = node_id_argument("bobcat.removeAttribute", arguments, 0)?;
        let name = string_argument("bobcat.removeAttribute", arguments, 1)?;
        let mut tree = borrow_tree("bobcat.removeAttribute", &tree)?;
        let document = tree.tree();
        validate_live_element(document, "bobcat.removeAttribute", node)?;
        document.remove_attribute(node, name);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "getAttribute", 2, move |arguments| {
        let node = node_id_argument("bobcat.getAttribute", arguments, 0)?;
        let name = string_argument("bobcat.getAttribute", arguments, 1)?;
        let mut tree = borrow_tree("bobcat.getAttribute", &tree)?;
        let document = tree.tree();
        validate_live_element(document, "bobcat.getAttribute", node)?;
        let value = document
            .get(node)
            .and_then(|node| node.attribute(name))
            .map(Arc::<str>::from);
        Ok(value.map_or(HostValue::Null, HostValue::String))
    })?;

    let tree = Rc::clone(handle);
    install(engine, "tagName", 1, move |arguments| {
        let node = node_id_argument("bobcat.tagName", arguments, 0)?;
        let mut tree = borrow_tree("bobcat.tagName", &tree)?;
        let document = tree.tree();
        validate_live_element(document, "bobcat.tagName", node)?;
        let tag = document
            .get(node)
            .and_then(dom::Node::tag_name)
            .ok_or_else(|| "bobcat.tagName requires a live element tag".to_owned())?;
        Ok(HostValue::String(Arc::from(tag)))
    })?;

    Ok(())
}

fn borrow_tree<'a>(
    function: &str,
    tree: &'a Rc<RefCell<TreeHandle>>,
) -> Result<RefMut<'a, TreeHandle>, String> {
    tree.try_borrow_mut()
        .map_err(|_| format!("{function} cannot re-enter the element tree"))
}

fn validate_live_element(
    document: &LynxDocument,
    function: &str,
    node: dom::NodeId,
) -> Result<(), String> {
    let node = document
        .get(node)
        .ok_or_else(|| format!("{function} received a stale element id"))?;
    if node.is_element() {
        Ok(())
    } else {
        Err(format!("{function} requires a live element id"))
    }
}

fn validate_removable(
    document: &LynxDocument,
    function: &str,
    node: dom::NodeId,
) -> Result<(), String> {
    let live = document
        .get(node)
        .ok_or_else(|| format!("{function} received a stale element id"))?;
    if live.is_document() || live.is_shadow_root() || node == document.document_element().id() {
        Err(format!(
            "{function} cannot remove the document, page root, or a shadow root"
        ))
    } else {
        Ok(())
    }
}

fn validate_insert(
    document: &LynxDocument,
    function: &str,
    parent: dom::NodeId,
    child: dom::NodeId,
    reference: Option<dom::NodeId>,
) -> Result<(), String> {
    validate_live_element(document, function, parent)?;
    validate_removable(document, function, child)?;
    if parent == child || document.is_ancestor(child, parent) {
        return Err(format!("{function} cannot create an element-tree cycle"));
    }
    if reference == Some(child) {
        return Err(format!(
            "{function} requires the reference and child to differ"
        ));
    }
    if let Some(reference) = reference {
        let reference_parent = document
            .get(reference)
            .ok_or_else(|| format!("{function} received a stale reference id"))?
            .parent_id();
        if reference_parent != Some(parent) {
            return Err(format!(
                "{function} requires the reference to be a child of the parent"
            ));
        }
    }
    Ok(())
}

fn validate_swap(
    document: &LynxDocument,
    function: &str,
    a: dom::NodeId,
    b: dom::NodeId,
) -> Result<(), String> {
    if a == b {
        return Err(format!("{function} requires distinct elements"));
    }
    validate_removable(document, function, a)?;
    validate_removable(document, function, b)?;
    if document.get(a).and_then(dom::Node::parent_id).is_none()
        || document.get(b).and_then(dom::Node::parent_id).is_none()
    {
        return Err(format!("{function} requires attached elements"));
    }
    if document.is_ancestor(a, b) || document.is_ancestor(b, a) {
        return Err(format!("{function} cannot swap an ancestor and descendant"));
    }
    Ok(())
}

fn argument(arguments: &[HostValue], index: usize) -> &HostValue {
    arguments.get(index).unwrap_or(&HostValue::Undefined)
}

/// The largest integer an `f64` represents exactly. A packed `NodeId` is built
/// to stay under it, so a handle survives the script boundary unchanged.
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

/// A `NodeId` crossing into script *is* the element's Lynx `unique_id`: the DOM
/// issues it, and `__GetElementUniqueID` hands back the same number the
/// creating PAPI returned.
///
/// The handle carries both the arena key and the generation that key was at, so
/// it crosses packed into one integer. The generation is what makes a stale
/// handle safe: script holds these for as long as it likes, and the collector
/// hands one back to `dropElement` after the element is gone, so an id that has
/// outlived its element must resolve to nothing rather than to whatever took
/// its place.
#[allow(
    clippy::cast_precision_loss,
    reason = "a packed handle is built to stay inside f64's exact-integer range"
)]
fn node_id_value(node: dom::NodeId) -> HostValue {
    HostValue::Number(node.to_bits() as f64)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bounds and integer checks above make the value a representable handle"
)]
fn node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<dom::NodeId, String> {
    let HostValue::Number(value) = *argument(arguments, index) else {
        return Err(format!("{function} expects a number for argument {index}"));
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value >= MAX_EXACT_INTEGER {
        return Err(format!(
            "{function} expects a non-negative integer node id for argument {index}"
        ));
    }
    dom::NodeId::from_bits(value as u64)
        .ok_or_else(|| format!("{function} got a number that is no element id: {value}"))
}

fn optional_node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<Option<dom::NodeId>, String> {
    match *argument(arguments, index) {
        HostValue::Undefined | HostValue::Null => Ok(None),
        _ => node_id_argument(function, arguments, index).map(Some),
    }
}

fn string_argument<'a>(
    function: &str,
    arguments: &'a [HostValue],
    index: usize,
) -> Result<&'a str, String> {
    match argument(arguments, index) {
        HostValue::String(value) => Ok(value),
        HostValue::Undefined | HostValue::Null => Ok(""),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}

#[cfg(all(test, feature = "quickjs"))]
mod tests {
    use super::*;
    use crate::tree::{PageConfig, Viewport, new_document};

    /// The handle a packed id names. A handle carries a generation as well as
    /// an arena key, so a test spells one the way script sees it — and for a
    /// document that has freed nothing the generation is zero, which is why
    /// these read as the small integers the PAPI hands out.
    fn node_id(bits: u64) -> dom::NodeId {
        dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
    }

    fn runtime() -> (MainThreadRuntime, SharedTree) {
        let elements = SharedTree::new(new_document(
            Viewport::new(393.0, 727.0),
            PageConfig::default(),
        ));
        let factory = crate::quickjs::engine_factory();
        let runtime = MainThreadRuntime::new(factory.as_ref(), elements.clone(), || {})
            .expect("main-thread runtime");
        (runtime, elements)
    }

    #[test]
    fn element_papi_boot_builds_the_private_tree() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateView(0));
                };
                ",
                "app:///main.js",
            )
            .expect("boot");

        assert!(elements.tree().get(node_id(3)).is_some());
    }

    #[test]
    fn bundle_url_reaches_script_error_location() {
        let (mut runtime, _elements) = runtime();
        let error = runtime
            .run_main_thread_script("const = 1", "app:///broken.js")
            .expect_err("syntax error");

        assert!(
            error
                .source
                .location
                .as_ref()
                .and_then(|location| location.source.as_deref())
                .is_some_and(|source| source == "app:///broken.js")
        );
    }

    #[test]
    fn stale_element_ids_become_script_errors_without_losing_the_tree() {
        let (mut runtime, elements) = runtime();
        let error = runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  bobcat.removeElement(999999);
                };
                ",
                "app:///invalid-tree-operation.js",
            )
            .expect_err("a stale id must be refused");

        assert!(error.source.message.contains("stale element id"));
        assert!(
            elements.try_tree().is_some(),
            "a rejected callback must return the private document to the presenter"
        );
    }

    /// The number script holds *is* the DOM's `NodeId` and the element's
    /// Lynx `unique_id` — one identity, issued by native — and dropping an
    /// element retires it. The element built afterwards reuses the freed
    /// node's storage but reports a different `unique_id`, so a handle that
    /// outlived its element can only ever name nothing.
    #[test]
    fn a_collected_element_retires_its_unique_id_instead_of_lending_it_out() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  let doomed = __CreateView(0);
                  __AppendElement(page, doomed);
                  if (__GetElementUniqueID(doomed) !== 3) {
                    throw new Error(
                      'the first element is node 3, got ' + __GetElementUniqueID(doomed),
                    );
                  }
                  __RemoveElement(page, doomed);
                  doomed = undefined;
                };
                ",
                "app:///collected.js",
            )
            .expect("main-thread script");
        assert!(
            elements.tree().get(node_id(3)).is_some(),
            "the detached element is still allocated while script could reach it"
        );

        runtime.collect_garbage().expect("collection");
        assert!(
            elements.tree().get(node_id(3)).is_none(),
            "a swept handle drops its element through the finalization registry"
        );

        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const replacement = __CreateView(0);
                  __AppendElement(page, replacement);
                  if (__GetElementUniqueID(replacement) === 3) {
                    throw new Error('a retired unique id was handed to a new element');
                  }
                };
                ",
                "app:///replacement.js",
            )
            .expect("main-thread script");
        assert!(
            elements.tree().get(node_id(3)).is_none(),
            "and the retired id keeps naming nothing"
        );
    }

    #[test]
    fn classes_attributes_and_identity_queries_reach_the_private_document() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetClasses(view, 'row bold');
                  __SetID(view, 'header');
                  __SetAttribute(view, 'flex-grow', 1);
                  if (__GetID(view) !== 'header') {
                    throw new Error('__GetID must read the id back, got ' + __GetID(view));
                  }
                  if (__GetTag(view) !== 'view' || __GetTag(page) !== 'page') {
                    throw new Error('__GetTag must report the Lynx tag');
                  }
                  if (__GetElementUniqueID(page) !== 2) {
                    throw new Error('the page is node 2, got ' + __GetElementUniqueID(page));
                  }
                };
                ",
                "app:///properties.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        let view = elements.get(node_id(3)).expect("the view is live");
        assert_eq!(view.classes().collect::<Vec<_>>(), ["row", "bold"]);
        assert_eq!(view.id_attribute(), Some("header"));
        assert_eq!(view.attribute("flex-grow"), Some("1"));
        assert_eq!(view.tag_name(), Some("view"));
    }

    #[test]
    fn clearing_a_class_id_or_attribute_removes_it_from_the_private_document() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetClasses(view, 'row');
                  __SetID(view, 'header');
                  __SetAttribute(view, 'text', 'hello');
                  __SetClasses(view, '');
                  __SetID(view, null);
                  __SetAttribute(view, 'text', undefined);
                  if (__GetID(view) !== null) {
                    throw new Error('__GetID must report null once the id is cleared');
                  }
                };
                ",
                "app:///clear-properties.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        let view = elements.get(node_id(3)).expect("the view is live");
        assert_eq!(view.classes().len(), 0);
        assert_eq!(view.id_attribute(), None);
        assert_eq!(view.attribute("text"), None);
    }

    #[test]
    fn inline_styles_reach_computed_style_and_layout() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const fromString = __CreateView(0);
                  const fromRecord = __CreateView(0);
                  __AppendElement(page, fromString);
                  __AppendElement(page, fromRecord);
                  __SetInlineStyles(fromString, 'width:10px;height:10px');
                  __SetInlineStyles(fromRecord, { width: '20px', height: '20px' });
                };
                ",
                "app:///inline-styles.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        for (id, expected) in [(node_id(3), 10.0_f32), (node_id(4), 20.0_f32)] {
            let layout = elements
                .rounded_layout(id)
                .expect("the styled view is laid out");
            assert!(
                (layout.size.width - expected).abs() < f32::EPSILON,
                "node {id} width {} should be {expected}",
                layout.size.width
            );
        }
    }

    #[test]
    fn record_inline_styles_are_fanned_out_by_name_before_reaching_stylo() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, {
                    paddingLeft: '4px',
                    '--accentColor': 'tomato',
                    color: null,
                    width: undefined,
                    definitelyNotAProperty: 'value',
                    height: 'not-a-length',
                  });
                };
                ",
                "app:///record-style.js",
            )
            .expect("main-thread script");
        let elements = elements.tree();
        let style = elements
            .get(node_id(3))
            .expect("the view is live")
            .attribute("style")
            .expect("valid single-property updates create an inline style");
        assert!(style.contains("padding-left: 4px"), "{style}");
        assert!(style.contains("--accentColor: tomato"), "{style}");
        assert!(!style.contains("definitely"), "{style}");
        assert!(!style.contains("height"), "{style}");
        assert!(
            !style
                .split(';')
                .any(|declaration| declaration.trim_start().starts_with("color:")),
            "{style}"
        );
    }

    #[test]
    fn a_later_inline_style_record_replaces_the_complete_declaration_block() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, { width: '10px', height: '20px' });
                  __SetInlineStyles(view, { height: '30px' });
                };
                ",
                "app:///replace-record-style.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        let view = elements.get(node_id(3)).expect("the view is live");
        let style = view.attribute("style").expect("height remains inline");
        assert!(!style.contains("width"), "{style}");
        assert!(style.contains("height: 30px"), "{style}");
        let layout = elements
            .rounded_layout(node_id(3))
            .expect("the view is laid out");
        assert!((layout.size.width - 393.0).abs() < f32::EPSILON);
        assert!((layout.size.height - 30.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clearing_inline_styles_removes_the_attribute_and_layout_effect() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, 'width:10px');
                  __SetInlineStyles(view, undefined);
                };
                ",
                "app:///clear-style.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        let view = elements.get(node_id(3)).expect("the view is live");
        assert_eq!(view.attribute("style"), None);
        let layout = elements
            .rounded_layout(node_id(3))
            .expect("the view is laid out");
        assert!(
            (layout.size.width - 393.0).abs() < f32::EPSILON,
            "the cleared width falls back to the page's, got {}",
            layout.size.width
        );
    }

    #[test]
    fn event_registrations_stay_in_the_realm() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  const worklet = { type: 'worklet', value: { _wkltId: '1:2' } };
                  __AddEvent(view, 'bindEvent', 'Tap', 'handler:1');
                  __AddEvent(view, 'bindEvent', 'Tap', worklet);
                  if (__GetEvent(view, 'tap', 'bindevent') !== 'handler:1') {
                    throw new Error('the background slot must survive the worklet one');
                  }
                  const events = __GetEvents(view);
                  if (events.length !== 2 || events[1].function !== worklet) {
                    throw new Error('both slots must be reported, got ' + events.length);
                  }
                  __AddEvent(view, 'bindEvent', 'tap', null);
                  if (__GetEvents(view).length !== 0) {
                    throw new Error('a null handler must clear both slots');
                  }
                };
                ",
                "app:///events.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        let view = elements.get(node_id(3)).expect("the view is live");
        assert_eq!(
            view.attributes().len(),
            0,
            "registration is realm bookkeeping, not a DOM mutation"
        );
    }

    #[test]
    fn update_list_info_is_refused_instead_of_becoming_an_attribute() {
        let (mut runtime, _elements) = runtime();
        let error = runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const list = __CreateList(0, function () {}, function () {});
                  __AppendElement(page, list);
                  __SetAttribute(list, 'update-list-info', { insertAction: [], removeAction: [] });
                };
                ",
                "app:///list.js",
            )
            .expect_err("the unimplemented list surface");

        assert!(error.to_string().contains("update-list-info"), "{error}");
    }
}
