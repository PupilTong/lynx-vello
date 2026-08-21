//! Engine-owned Lynx main-thread runtime over an injected JavaScript VM.

use std::cell::{Cell, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use dom::event::EventSteps;

use crate::engine::{SharedListenerNames, SharedTree};
use crate::script::{HostValue, ScriptEngine, ScriptEngineFactory, ScriptError};
use crate::tree::LynxDocument;
use crate::tree::raw_text::drop_element_and_owned_text;

const BOOT_SOURCE_NAME: &str = "<lynx boot>";
const ELEMENT_PAPI_SOURCE_NAME: &str = "element-papi.js";
const MAIN_THREAD_GLOBALS_SOURCE_NAME: &str = "main-thread-globals.js";

const ELEMENT_PAPI_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/element-papi.js");
const MAIN_THREAD_GLOBALS_SOURCE: &str = include_str!("main-thread-globals.js");

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

/// The nodes a walk should visit for one event name: `(node, is capture pass)`.
type ListenerNodes = HashSet<(dom::NodeId, bool)>;

/// What the realm has told the host about listeners, and what it tells it
/// during a walk.
///
/// Shared with the host functions that maintain it, so it is `Rc` rather than
/// owned: `bobcat.enableEventListener` and the dispatch driver are different
/// stack frames on the same thread.
#[derive(Default)]
struct EventState {
    /// The nodes the realm has a listener on, per event name and pass. Keyed
    /// by name first so a walk resolves it once and then tests each step
    /// without touching the name again — and so an event no listener wants
    /// costs one lookup for the whole walk.
    listeners: RefCell<HashMap<Arc<str>, ListenerNodes>>,
    /// Set by `bobcat.stopPropagation`. A pure flag write: the realm is inside
    /// a `call_host_member` when it runs, and re-entering the realm from a
    /// host function would nest an execution guard, which `QuickJS` refuses.
    stopped: Cell<bool>,
}

/// The private main-thread runtime used by the engine pipeline.
pub(crate) struct MainThreadRuntime {
    engine: Box<dyn ScriptEngine>,
    tree: Rc<RefCell<TreeHandle>>,
    events: Rc<EventState>,
    /// Names one dispatch, so the realm can keep one event object alive across
    /// the whole walk instead of minting one per node. Not shared with the
    /// host functions: only [`Self::dispatch_event`] reads or advances it, and
    /// it holds `&mut self` while it does.
    next_event_id: u32,
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
        listener_names: Arc<SharedListenerNames>,
        on_flush: impl Fn() + 'static,
    ) -> Result<Self, MainThreadError> {
        let mut engine = factory
            .create()
            .map_err(|error| MainThreadError::from_engine("creating the script VM", error))?;
        let events = Rc::new(EventState::default());
        let tree = install_bobcat(engine.as_mut(), elements, listener_names, on_flush, &events)?;
        Ok(Self {
            engine,
            tree,
            events,
            next_event_id: 0,
        })
    }

    /// Delivers a path the presenting side already computed.
    ///
    /// The split is deliberate. The presenting thread holds the document when
    /// it routes an input event, so building the path there costs it no extra
    /// borrow, and it must stay responsive — a long task here cannot be
    /// allowed to stop scrolling. This thread owns the realm, which cannot
    /// move, so delivery has to happen here.
    ///
    /// Nothing has to guard the window in between. A `NodeId` names one node
    /// for the life of the document — freeing retires it — so a step whose
    /// node was freed since the path was built resolves to no handle and
    /// reaches no one, which is exactly what should happen.
    ///
    /// Each walk carries an id naming it, and each call says whether it is the
    /// last. Together they let the realm hold one event object for the whole
    /// dispatch — which is what makes a property a listener writes visible to
    /// the next one, as a real `Event` does — without the host retaining
    /// anything of the realm's.
    ///
    /// Returns whether anything was delivered.
    pub(crate) fn dispatch_event(
        &mut self,
        steps: &EventSteps,
        name: &str,
        detail_json: &str,
    ) -> Result<bool, MainThreadError> {
        let name: Arc<str> = Arc::from(name);

        // One lookup for the whole walk, and the first thing done: an event no
        // listener registered for never reaches the realm, never touches the
        // name again, and never takes the document.
        let nodes = {
            let listeners = self.events.listeners.borrow();
            match listeners.get(&name) {
                Some(nodes) => nodes.clone(),
                None => return Ok(false),
            }
        };

        self.events.stopped.set(false);

        // Fresh per dispatch, and never reused by a live one: dispatch takes
        // `&mut self`, so a listener cannot start a second walk from inside
        // this one, and the realm drops its entry before this call returns.
        // The wrap is therefore unreachable rather than merely unlikely.
        let event_id = self.next_event_id;
        self.next_event_id = self.next_event_id.wrapping_add(1);

        // Filtered ahead of the walk so each call can say whether another
        // follows it. That is the realm's cue to drop the event object, and
        // the only one the host can give: the walk's other two endings — a
        // listener stopping propagation, a listener throwing — are both
        // visible to the realm itself as they happen.
        let mut deliverable = steps
            .steps()
            .iter()
            .filter(|step| nodes.contains(&(step.node, step.capture)))
            .peekable();

        let mut delivered = false;
        while let Some(step) = deliverable.next() {
            if self.events.stopped.get() {
                break;
            }
            let arguments = [
                node_id_value(step.node),
                node_id_value(step.target),
                HostValue::Number(f64::from(u8::from(step.capture))),
                HostValue::String(Arc::clone(&name)),
                HostValue::String(Arc::from(detail_json)),
                HostValue::Number(f64::from(event_id)),
                HostValue::Boolean(deliverable.peek().is_none()),
            ];
            let called =
                self.engine
                    .call_host_member("bobcat", "event_listener_callback", &arguments);
            // Before anything else, including propagating a failure. Building
            // the event object alone takes the document — it reads two ids —
            // so a listener that merely throws would otherwise strand it in
            // the hand-off slot, and nothing could ever put it back: the only
            // code that releases needs a next dispatch, and a next dispatch is
            // only built while the tree is held.
            self.tree.borrow_mut().release();
            if !called
                .map_err(|error| MainThreadError::from_engine("delivering an event", error))?
            {
                // The realm published no callback; nothing on this path will.
                break;
            }
            delivered = true;
        }
        Ok(delivered)
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
    listener_names: Arc<SharedListenerNames>,
    on_flush: impl Fn() + 'static,
    events: &Rc<EventState>,
) -> Result<Rc<RefCell<TreeHandle>>, MainThreadError> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        slot: elements,
        taken: None,
    }));

    install_bobcat_object(
        engine,
        &handle,
        on_flush,
        events,
        Arc::clone(&listener_names),
    )?;
    install_event_members(engine, events, listener_names)?;
    engine
        .execute_script(MAIN_THREAD_GLOBALS_SOURCE, MAIN_THREAD_GLOBALS_SOURCE_NAME)
        .map_err(|error| {
            MainThreadError::from_engine("installing the main-thread globals", error)
        })?;
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

/// Installs `bobcat.<name>` members that parse their arguments, borrow the
/// tree, and run against the private document. Each `$parser` is one of the
/// argument helpers below, applied at the argument's position; `NAME` is the
/// diagnostic prefix every helper and validator stitches into its error.
macro_rules! tree_members {
    ($engine:ident, $handle:ident; $(
        fn $name:ident($($arg:ident: $parser:ident),*) |$document:ident| $body:block
    )*) => {$({
        const NAME: &str = concat!("bobcat.", stringify!($name));
        let tree = Rc::clone($handle);
        let arity = 0u8 $(+ { let _ = stringify!($arg); 1u8 })*;
        install($engine, stringify!($name), arity, move |arguments| {
            #[allow(unused_mut, reason = "zero-argument members never advance it")]
            let mut index = 0usize;
            $(
                let $arg = $parser(NAME, arguments, index)?;
                index += 1;
            )*
            let _ = (arguments, index);
            let mut handle = borrow_tree(NAME, &tree)?;
            let $document = handle.tree();
            $body
        })?;
    })*};
}

fn install_bobcat_object(
    engine: &mut dyn ScriptEngine,
    handle: &Rc<RefCell<TreeHandle>>,
    on_flush: impl Fn() + 'static,
    events: &Rc<EventState>,
    listener_names: Arc<SharedListenerNames>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, handle;
        fn createPage() |document| {
            Ok(node_id_value(document.document_element().id()))
        }
        fn createElement(tag: string_argument) |document| {
            Ok(node_id_value(document.create_element(tag, ())))
        }
        fn parentNode(node: node_id_argument) |document| {
            let parent = document.get(node).and_then(dom::Node::parent_id);
            Ok(parent.map_or(HostValue::Null, node_id_value))
        }
        fn insertBefore(
            parent: node_id_argument,
            child: node_id_argument,
            reference: optional_node_id_argument
        ) |document| {
            validate_insert(document, NAME, parent, child, reference)?;
            document.insert_before(parent, child, reference);
            Ok(HostValue::Undefined)
        }
        fn removeElement(child: node_id_argument) |document| {
            validate_removable(document, NAME, child)?;
            document.remove_element(child);
            Ok(HostValue::Undefined)
        }
        fn replaceElement(
            new_element: node_id_argument,
            old_element: node_id_argument
        ) |document| {
            validate_removable(document, NAME, old_element)?;
            validate_live_element(document, NAME, new_element)?;
            if let Some(parent) = document.get(old_element).and_then(dom::Node::parent_id) {
                validate_insert(document, NAME, parent, new_element, Some(old_element))?;
                document.insert_before(parent, new_element, Some(old_element));
                document.remove_element(old_element);
            }
            Ok(HostValue::Undefined)
        }
        fn swapElement(a: node_id_argument, b: node_id_argument) |document| {
            validate_swap(document, NAME, a, b)?;
            document.swap_element(a, b);
            Ok(HostValue::Undefined)
        }
    }

    install_attribute_members(engine, handle)?;

    let tree = Rc::clone(handle);
    let state = Rc::clone(events);
    install(engine, "dropElement", 1, move |arguments| {
        let node = node_id_argument("bobcat.dropElement", arguments, 0)?;
        let mut tree = borrow_tree("bobcat.dropElement", &tree)?;
        let document = tree.tree();
        validate_removable(document, "bobcat.dropElement", node)?;
        drop_element_and_owned_text(document, node);
        state.listeners.borrow_mut().retain(|name, nodes| {
            nodes.retain(|(id, _)| {
                let keep = *id != node;
                if !keep {
                    // The purge is a removal like any other: the shared
                    // name table must not keep counting a dead registration.
                    listener_names.note_disabled(name);
                }
                keep
            });
            !nodes.is_empty()
        });
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

/// Installs the three members the realm's `EventTarget` speaks to.
///
/// None of them touches the document. The first two only maintain an index —
/// which nodes are worth visiting — and the third only sets a flag. That is
/// not an optimization: `stopPropagation` runs while the host is inside a call
/// into the realm, and re-entering the realm from a host function would nest
/// an execution guard, which `QuickJS` refuses.
fn install_event_members(
    engine: &mut dyn ScriptEngine,
    events: &Rc<EventState>,
    listener_names: Arc<SharedListenerNames>,
) -> Result<(), MainThreadError> {
    let state = Rc::clone(events);
    let names = Arc::clone(&listener_names);
    install(engine, "enableEventListener", 3, move |arguments| {
        let node = node_id_argument("bobcat.enableEventListener", arguments, 0)?;
        let capture = capture_argument("bobcat.enableEventListener", arguments, 1)?;
        let name = string_argument("bobcat.enableEventListener", arguments, 2)?;
        if state
            .listeners
            .borrow_mut()
            .entry(Arc::from(name))
            .or_default()
            .insert((node, capture))
        {
            // Mirrored to the presenting thread only on true transitions, so
            // the shared table counts registrations, never repeats.
            names.note_enabled(name);
        }
        Ok(HostValue::Undefined)
    })?;

    let state = Rc::clone(events);
    let names = listener_names;
    install(engine, "disableEventListener", 3, move |arguments| {
        let node = node_id_argument("bobcat.disableEventListener", arguments, 0)?;
        let capture = capture_argument("bobcat.disableEventListener", arguments, 1)?;
        let name = string_argument("bobcat.disableEventListener", arguments, 2)?;
        let mut listeners = state.listeners.borrow_mut();
        if let Some(nodes) = listeners.get_mut(name) {
            if nodes.remove(&(node, capture)) {
                names.note_disabled(name);
            }
            if nodes.is_empty() {
                listeners.remove(name);
            }
        }
        Ok(HostValue::Undefined)
    })?;

    let state = Rc::clone(events);
    install(engine, "stopPropagation", 0, move |_arguments| {
        state.stopped.set(true);
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
    tree_members! { engine, handle;
        fn setAttribute(
            node: node_id_argument,
            name: string_argument,
            value: string_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            document.set_attribute(node, name, value);
            Ok(HostValue::Undefined)
        }
        // Deliberately name-based: this PAPI receives record keys, custom
        // properties have no numeric id, and Stylo's internal PropertyId is
        // not a stable script ABI. A future numeric-key `__AddInlineStyle`
        // can translate its bundle id in JavaScript/the decoder-owned layer
        // before reaching this one primitive.
        fn set_node_property(
            node: node_id_argument,
            name: string_argument,
            value: string_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            document.set_inline_style_property(node, name, value);
            Ok(HostValue::Undefined)
        }
        fn removeAttribute(node: node_id_argument, name: string_argument) |document| {
            validate_live_element(document, NAME, node)?;
            document.remove_attribute(node, name);
            Ok(HostValue::Undefined)
        }
        fn getAttribute(node: node_id_argument, name: string_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let value = document
                .get(node)
                .and_then(|node| node.attribute(name))
                .map(Arc::<str>::from);
            Ok(value.map_or(HostValue::Null, HostValue::String))
        }
        fn tagName(node: node_id_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let tag = document
                .get(node)
                .and_then(dom::Node::tag_name)
                .ok_or_else(|| "bobcat.tagName requires a live element tag".to_owned())?;
            Ok(HostValue::String(Arc::from(tag)))
        }
    }

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

/// The `type_id` the realm registers with: `0` bubble, `1` capture.
fn capture_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match *argument(arguments, index) {
        HostValue::Number(0.0) => Ok(false),
        HostValue::Number(1.0) => Ok(true),
        _ => Err(format!("{function} expects 0 or 1 for argument {index}")),
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

    /// The path the presenting side would compute for `target`.
    fn steps(elements: &SharedTree, target: u64) -> EventSteps {
        elements.tree().event_steps(node_id(target), true, true)
    }

    fn runtime() -> (MainThreadRuntime, SharedTree) {
        runtime_over(new_document(
            Viewport::new(393.0, 727.0),
            PageConfig::default(),
        ))
    }

    /// The same runtime over a document that can shape text: Ahem's solid em
    /// squares make a run's box its glyph count times its font size.
    fn text_runtime() -> (MainThreadRuntime, SharedTree) {
        const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

        let mut document = new_document(Viewport::new(393.0, 727.0), PageConfig::default());
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        runtime_over(document)
    }

    fn runtime_over(document: LynxDocument) -> (MainThreadRuntime, SharedTree) {
        let elements = SharedTree::new(document);
        let factory = crate::quickjs::engine_factory();
        let runtime = MainThreadRuntime::new(
            factory.as_ref(),
            elements.clone(),
            Arc::new(SharedListenerNames::default()),
            || {},
        )
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
    fn main_thread_globals_supply_shape_only_runtime_bridges() {
        let (mut runtime, _elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                if (lynx.SystemInfo !== SystemInfo || !Object.isFrozen(SystemInfo)) {
                  throw new Error('SystemInfo must be one frozen shared snapshot');
                }
                if (lynx.__globalProps !== __globalProps) {
                  throw new Error('the bare and lynx global props must share identity');
                }
                if (lynx.__initData === null || typeof lynx.__initData !== 'object') {
                  throw new Error('init data must start as an empty object');
                }
                if (!('NativeModules' in globalThis) || NativeModules !== undefined) {
                  throw new Error('the main-thread native-module sentinel must be undefined');
                }
                if (typeof lynxCoreInject !== 'undefined') {
                  throw new Error('the background-thread injection must not leak into this realm');
                }
                if (typeof globDynamicComponentEntry !== 'undefined') {
                  throw new Error('the dynamic-chunk entry must not leak into the card realm');
                }
                if (typeof __SetCSSId !== 'function' ||
                    __SetCSSId([], 0, 'entry') !== undefined) {
                  throw new Error('the scoped-style PAPI must accept its call and record nothing');
                }

                const core = lynx.getCoreContext();
                const js = lynx.getJSContext();
                const native = lynx.getNative();
                if (core === js || core === native || js === native) {
                  throw new Error('the three context directions must remain distinct');
                }
                for (const [name, context, again] of [
                  ['core', core, lynx.getCoreContext()],
                  ['js', js, lynx.getJSContext()],
                  ['native', native, lynx.getNative()]
                ]) {
                  if (context !== again) {
                    throw new Error(name + ' context identity must be stable');
                  }
                  context.postMessage({});
                  context.addEventListener('ignored', function () {});
                  context.removeEventListener('ignored', function () {});
                  if (context.dispatchEvent({ type: 'ignored', data: {} }) !== 3) {
                    throw new Error(name + ' context must report a suppressed event');
                  }
                }

                const emitter = lynx.getJSModule('GlobalEventEmitter');
                if (emitter !== lynx.getJSModule('GlobalEventEmitter') ||
                    lynx.getJSModule('missing') !== undefined) {
                  throw new Error('only the stable empty global-event module is exposed');
                }
                for (const method of [
                  'addListener', 'removeListener', 'removeAllListeners',
                  'emit', 'trigger', 'toggle'
                ]) {
                  emitter[method]('ignored', function () {});
                }

                if (lynx.performance.isProfileRecording() !== false ||
                    lynx.performance.profileFlowId() !== 0 ||
                    lynx.performance._generatePipelineOptions() !== undefined) {
                  throw new Error('the performance shell must stay inert');
                }
                _AddEventListener('ignored', function () {});
                _ReportError(new Error('ignored'));
                _SetSourceMapRelease({ release: 'ignored' });
                __OnLifecycleEvent(['ignored', {}]);

                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                };
                ",
                "app:///runtime-globals.js",
            )
            .expect("shape-only main-thread globals");
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
    fn a_dispatch_reaches_only_the_nodes_that_registered_a_listener() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  // A registration is weak by its handle, so an app that wants
                  // its listeners to survive holds its elements. ReactLynx's
                  // snapshot instances do; this stands in for them.
                  globalThis.held = [page, outer, inner];
                  const note = (label) => (event) =>
                    seen.push(label + ':' + event.currentTarget.uid + ':' + event.eventPhase);
                  __AddEventListener(page, 'tap', note('page-capture'), { capture: true });
                  __AddEventListener(inner, 'tap', note('inner'), {});
                  // `outer` registers nothing, so the walk must skip it.
                };
                ",
                "app:///listeners.js",
            )
            .expect("main-thread script");

        let target = 4;
        let delivered = runtime
            .dispatch_event(&steps(&elements, target), "tap", "{\"x\":1}")
            .expect("dispatch");
        assert!(delivered);

        runtime
            .evaluate(
                r"
                if (seen.join('|') !== 'page-capture:2:1|inner:4:2') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                ",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");
    }

    #[test]
    fn add_event_registers_against_the_real_index_and_a_catch_form_ends_the_walk() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.seen = [];
                // A card's own worklet runtime installs this; `__AddEvent`
                // reaches for it per delivery, since a worklet is the only
                // handler kind that runs in this realm.
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  globalThis.held = [page, outer, inner];
                  const note = (label) => ({
                    type: 'worklet',
                    value: {
                      body: (event) =>
                        seen.push(label + ':' + event.currentTarget.uid),
                    },
                  });
                  // A catch form on the target, a plain bind on its ancestor:
                  // the second must never be reached, and only the host can
                  // decide that, from the `stopPropagation` the catch causes.
                  __AddEvent(inner, 'catchEvent', 'tap', note('inner-catch'));
                  __AddEvent(outer, 'bindEvent', 'tap', note('outer-bind'));
                  // The same node, same name, other pass: a separate index
                  // entry, and one the bubble walk must not reach.
                  __AddEvent(page, 'capture-bind', 'tap', note('page-capture'));
                };
                ",
                "app:///handlers.js",
            )
            .expect("main-thread script");

        assert!(
            runtime
                .dispatch_event(&steps(&elements, 4), "tap", "")
                .expect("dispatch")
        );

        runtime
            .evaluate(
                r"
                if (seen.join('|') !== 'page-capture:2|inner-catch:4') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                ",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");
    }

    #[test]
    fn a_replaced_add_event_handler_moves_its_node_between_passes() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.seen = [];
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const inner = __CreateView(0);
                  __AppendElement(page, inner);
                  globalThis.held = [page, inner];
                  const note = (label) => ({
                    type: 'worklet',
                    value: { body: () => seen.push(label) },
                  });
                  // One name, one entry: the second call replaces the first
                  // outright, which also moves the node's index entry from the
                  // bubble pass to the capture one.
                  __AddEvent(inner, 'bindEvent', 'tap', note('bubble'));
                  __AddEvent(inner, 'capture-bind', 'tap', note('capture'));
                };
                ",
                "app:///handlers.js",
            )
            .expect("main-thread script");

        assert!(
            runtime
                .dispatch_event(&steps(&elements, 3), "tap", "")
                .expect("dispatch")
        );

        runtime
            .evaluate(
                r"
                if (seen.join('|') !== 'capture') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                if (__GetEvent(held[1], 'tap', 'bindEvent') !== undefined) {
                  throw new Error('the replaced form must not still answer');
                }
                // Removing it leaves the node out of the index entirely, so a
                // further dispatch reaches nobody at all.
                __AddEvent(held[1], 'capture-bind', 'tap', undefined);
                ",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");

        assert!(
            !runtime
                .dispatch_event(&steps(&elements, 3), "tap", "")
                .expect("dispatch")
        );
    }

    #[test]
    fn one_id_names_a_whole_walk_and_only_its_last_delivery_is_flagged() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  globalThis.held = [page, outer, inner];
                  __AddEventListener(page, 'tap', () => {}, { capture: true });
                  __AddEventListener(inner, 'tap', () => {}, {});
                };
                ",
                "app:///listeners.js",
            )
            .expect("main-thread script");

        // Replacing the realm's own callback is how the host's half of the
        // contract becomes observable: what the realm does with these numbers
        // is its business, but the numbers themselves are the host's.
        runtime
            .evaluate(
                r"
                globalThis.calls = [];
                bobcat.event_listener_callback = (node, target, phase, name,
                                                  detail, eventId, isLastCall) => {
                  calls.push([node, eventId, isLastCall]);
                };
                ",
                "app:///record.js",
                "installing the recorder",
            )
            .expect("recorder");

        for _ in 0..2 {
            assert!(
                runtime
                    .dispatch_event(&steps(&elements, 4), "tap", "")
                    .expect("dispatch")
            );
        }

        runtime
            .evaluate(
                r"
                const shape = calls.map((call) => call.join(':')).join('|');
                // Two deliveries per walk: `outer` registered nothing, so the
                // flag has to land on the last *delivered* step, not on the
                // last step of the path.
                if (shape !== '2:0:false|4:0:true|2:1:false|4:1:true') {
                  throw new Error('unexpected walk shape: ' + shape);
                }
                ",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");
    }

    #[test]
    fn a_listener_may_mutate_the_tree_it_was_dispatched_on() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __AddEventListener(view, 'tap', () => {
                    // The document is back in its slot while this runs, which
                    // is the whole reason the path is computed up front.
                    __SetAttribute(view, 'tapped', 'yes');
                  }, {});
                };
                ",
                "app:///mutate.js",
            )
            .expect("main-thread script");

        runtime
            .dispatch_event(&steps(&elements, 3), "tap", "")
            .expect("dispatch");

        assert_eq!(
            elements
                .tree()
                .get(node_id(3))
                .expect("the view is live")
                .attribute("tapped"),
            Some("yes")
        );
    }

    #[test]
    fn an_unrelated_element_being_collected_does_not_truncate_the_walk() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  // Deliberately unheld: this is the element a sweep collects.
                  const doomed = __CreateView(0);
                  __AppendElement(page, doomed);
                  globalThis.doomed = __GetElementUniqueID(doomed);
                  globalThis.held = [page, view];
                  __AddEventListener(page, 'tap', () => seen.push('page'), { capture: true });
                  __AddEventListener(view, 'tap', () => seen.push('view'), {});
                };
                ",
                "app:///collect.js",
            )
            .expect("main-thread script");

        let steps = steps(&elements, 3);
        // Free the unrelated element the way a finalizer would, between the
        // path being built and the walk running.
        runtime
            .evaluate("bobcat.dropElement(doomed);", "app:///sweep.js", "sweeping")
            .expect("sweep");

        runtime.dispatch_event(&steps, "tap", "").expect("dispatch");

        // A collected handle is routine — a ReactLynx re-render drops them
        // constantly — so it must not silently cost the rest of the walk.
        runtime
            .evaluate(
                "if (seen.join('|') !== 'page|view') throw new Error('truncated: ' + seen.join('|'));",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");
    }

    #[test]
    fn stopping_propagation_ends_the_walk() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __AddEventListener(page, 'tap', (event) => {
                    seen.push('page');
                    __StopPropagation(event);
                  }, { capture: true });
                  __AddEventListener(view, 'tap', () => seen.push('view'), {});
                };
                ",
                "app:///stop.js",
            )
            .expect("main-thread script");

        runtime
            .dispatch_event(&steps(&elements, 3), "tap", "")
            .expect("dispatch");

        runtime
            .evaluate(
                "if (seen.join('|') !== 'page') throw new Error('got ' + seen.join('|'));",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");
    }

    #[test]
    fn a_document_whose_script_registered_nothing_never_enters_the_realm() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateView(0));
                };
                ",
                "app:///quiet.js",
            )
            .expect("main-thread script");

        assert!(
            !runtime
                .dispatch_event(&steps(&elements, 3), "tap", "")
                .expect("dispatch"),
            "with an empty listener index the walk crosses the boundary zero times"
        );
    }

    #[test]
    fn a_raw_text_reaches_the_private_document_as_a_laid_out_run() {
        let (mut runtime, elements) = text_runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetInlineStyles(text, 'font-family:Ahem;font-size:20px');
                  __AppendElement(text, __CreateRawText('hello'));
                  __AppendElement(page, text);
                };
                ",
                "app:///raw-text.js",
            )
            .expect("main-thread script");

        let tree = elements.tree();
        let carrier = tree.get(node_id(4)).expect("the raw-text is live");
        assert_eq!(carrier.tag_name(), Some("raw-text"));
        assert_eq!(carrier.attribute("text"), Some("hello"));
        let run = carrier.first_child().expect("the reflected run").id();
        assert_eq!(tree.get(run).and_then(dom::Node::text), Some("hello"));

        let layout = tree.rounded_layout(run).expect("the run is laid out");
        assert!(
            (layout.size.width - 100.0).abs() < f32::EPSILON
                && (layout.size.height - 20.0).abs() < f32::EPSILON,
            "five Ahem em squares at 20px, got {:?}",
            layout.size
        );
        assert!(
            tree.rounded_layout(node_id(3))
                .is_some_and(|text| (text.size.height - 20.0).abs() < f32::EPSILON),
            "and the text element is sized by the run it contains"
        );
    }

    #[test]
    fn rewriting_the_text_attribute_relays_out_the_same_run() {
        let (mut runtime, elements) = text_runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetInlineStyles(text, 'font-family:Ahem;font-size:20px');
                  const raw = __CreateRawText('hello');
                  __AppendElement(text, raw);
                  __AppendElement(page, text);
                  __SetAttribute(raw, 'text', 'hi');
                };
                ",
                "app:///update-raw-text.js",
            )
            .expect("main-thread script");

        let tree = elements.tree();
        let run = tree
            .get(node_id(4))
            .and_then(dom::Node::first_child)
            .expect("the reflected run")
            .id();
        assert_eq!(
            run,
            node_id(5),
            "the update re-points the run it already had"
        );
        assert_eq!(tree.get(run).and_then(dom::Node::text), Some("hi"));
        assert!(
            tree.rounded_layout(run)
                .is_some_and(|layout| (layout.size.width - 40.0).abs() < f32::EPSILON),
            "the shorter run is re-measured, not left at its old width"
        );
    }

    #[test]
    fn a_collected_raw_text_takes_its_run_with_it() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __AppendElement(page, text);
                  let raw = __CreateRawText('hello');
                  __AppendElement(text, raw);
                  __RemoveElement(text, raw);
                  raw = undefined;
                };
                ",
                "app:///collected-raw-text.js",
            )
            .expect("main-thread script");
        assert!(
            elements
                .tree()
                .get(node_id(4))
                .and_then(dom::Node::first_child)
                .is_some(),
            "the detached carrier still holds its run"
        );

        runtime.collect_garbage().expect("collection");

        let tree = elements.tree();
        assert!(tree.get(node_id(4)).is_none(), "the carrier is freed");
        assert!(
            tree.get(node_id(5)).is_none(),
            "and so is the run's node, which no handle could ever have named"
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
