//! Engine-owned Lynx main-thread runtime over the core's `QuickJS` realm.

use std::cell::{Cell, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use smallvec::SmallVec;

use crate::quickjs::ScriptEngine;
use crate::script::ScriptError;
use crate::tree::LynxDocument;
use crate::view::{FrameHub, SharedListenerNames};

const BOOT_MODULE_SPECIFIER: &str = "bobcat:boot";
const ELEMENT_MODULE_SPECIFIER: &str = "bobcat:element";
const HOST_MODULE_SPECIFIER: &str = "bobcat-internal:host";
const RUNTIME_MODULE_SPECIFIER: &str = "bobcat:runtime";
const EVENT_DISPATCH_EXPORT: &str = "__BobcatDispatchEvent";

/// Declarations one `__SetInlineStyles` record carries without touching the
/// heap. Compiled `ReactLynx` records are a handful of properties.
const INLINE_DECLARATIONS: usize = 16;

/// Registrations one node carries without touching the heap. A `ReactLynx`
/// element with more than four distinct listener kinds is unusual.
const INLINE_NODE_LISTENERS: usize = 4;

/// Steps of one event path delivered without touching the heap. Deeper paths
/// exist; a path with more than this many *listening* nodes does not.
const INLINE_DELIVERIES: usize = 8;

const ELEMENT_PAPI_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/element-papi.mjs");
const RUNTIME_MODULE_SOURCE: &str =
    include_str!("../../../packages/bobcat-element/src/main-thread-runtime.mjs");

pub(crate) const ENTRY_PREAMBLE: &str = r#"import {
  lynx,
  SystemInfo,
  __globalProps,
  NativeModules,
  _AddEventListener,
  _ReportError,
  _SetSourceMapRelease,
  __OnLifecycleEvent,
} from "bobcat:runtime";
import {
  __CreatePage,
  __CreateElement,
  __CreateWrapperElement,
  __CreateText,
  __CreateImage,
  __CreateView,
  __CreateScrollView,
  __CreateRawText,
  __CreateList,
  __AppendElement,
  __InsertElementBefore,
  __RemoveElement,
  __ReplaceElement,
  __ReplaceElements,
  __SwapElement,
  __SetClasses,
  __SetID,
  __GetID,
  __GetTag,
  __GetChildren,
  __GetAttributeByName,
  __GetAttributeNames,
  __GetElementUniqueID,
  __SetInlineStyles,
  __SetCSSId,
  __SetAttribute,
  __UpdateListCallbacks,
  __AddEvent,
  __GetEvent,
  __GetEvents,
  __SetEvents,
  __AddEventListener,
  __RemoveEventListener,
  __StopPropagation,
  __StopImmediatePropagation,
  __FlushElementTree,
} from "bobcat:element";
//# allFunctionsCalledOnLoad
"#;

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

    pub(crate) fn into_script_error(mut self) -> ScriptError {
        self.source.message = Arc::from(format!("{}: {}", self.context, self.source.message));
        self.source
    }
}

impl fmt::Display for MainThreadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for MainThreadError {}

/// How many removals go by between collections.
///
/// A removal is where a subtree's handles start dying — `ReactLynx` unmounts
/// with `__RemoveElement` and then deletes the snapshot's element list, which
/// drops the last reference to the detached root's handle and, through the
/// child sets under it, to every handle in the subtree — and nothing frees
/// the elements until those handles are finalized. A handle reachable only
/// from a registration it captured is a cycle, which only a collection
/// resolves. `QuickJS` collects on its own only at allocation pressure (its
/// threshold is 1.5× the live size after each collection), so a card that
/// detaches steadily while allocating little keeps dead subtrees for a long
/// time: measured, a 301-node subtree outlived 30 batches at 5 elements of
/// churn per batch, and an idle card never freed it. Counting removals is the
/// cheapest signal that correlates with that garbage; the count is a policy
/// knob, not a measurement.
const REMOVALS_PER_COLLECTION: u32 = 32;

/// The main thread's outright ownership of the document, plus the publish
/// seam its commits leave through.
struct TreeHandle {
    document: LynxDocument,
    /// Removals since the last collection; see [`REMOVALS_PER_COLLECTION`].
    removals: u32,
    /// Where committed frames are published.
    hub: Arc<FrameHub>,
    /// Wakes the presenting side after a publish, through whatever frame
    /// capability is currently attached.
    wake: Box<dyn Fn()>,
}

impl TreeHandle {
    fn tree(&mut self) -> &mut LynxDocument {
        &mut self.document
    }

    /// Runs the whole pipeline and publishes the committed frame — the
    /// native half of `__FlushElementTree`, and the only place frames leave
    /// this thread.
    fn flush(&mut self) {
        let frame = self.document.commit();
        self.hub.publish(frame);
        (self.wake)();
    }

    /// Commits and publishes only when something is stale — the tail of
    /// every served command round, which is what makes "we do not guarantee
    /// the tree is not flushed outside `__FlushElementTree`" true.
    fn commit_if_dirty(&mut self) {
        if self.document.needs_render() {
            self.flush();
        }
    }

    /// Notes that a subtree left the tree.
    fn note_removal(&mut self) {
        self.removals = self.removals.saturating_add(1);
    }

    /// Whether enough removals have accumulated for a collection, resetting
    /// the count when they have.
    fn take_collection_due(&mut self) -> bool {
        if self.removals < REMOVALS_PER_COLLECTION {
            return false;
        }
        self.removals = 0;
        true
    }
}

/// The nodes a walk should visit for one event name: `(node, is capture pass)`.
type ListenerNodes = HashSet<(dom::NodeId, bool)>;

/// The `(name, is capture pass)` pairs one node carries listeners for.
type NodeListeners = SmallVec<[(Arc<str>, bool); INLINE_NODE_LISTENERS]>;

/// What the realm has told the host about listeners, and what it tells it
/// during a walk.
///
/// Shared with the host functions that maintain it, so it is `Rc` rather than
/// owned: the native `enableEventListener` export and the dispatch driver are
/// different stack frames on the same thread.
struct EventState {
    /// The nodes the realm has a listener on, per event name and pass. Keyed
    /// by name first so a walk resolves it once and then tests each step
    /// without touching the name again — and so an event no listener wants
    /// costs one lookup for the whole walk.
    listeners: RefCell<HashMap<Arc<str>, ListenerNodes>>,
    /// The same registrations keyed the other way, so dropping an element
    /// costs its own listeners rather than a scan of every name.
    by_node: RefCell<HashMap<dom::NodeId, NodeListeners>>,
    /// The presenting thread's view of which names are registered anywhere.
    ///
    /// Maintained here rather than at a batch boundary because it is what the
    /// realm has just been told, and touched only on a true transition: a
    /// second listener for a name already carried moves no count.
    names: Arc<SharedListenerNames>,
    /// Set by the native `stopPropagation` export. A pure flag write: the
    /// realm is inside a `call_module_export` when it runs, and re-entering
    /// the realm from a host function would nest an execution guard, which
    /// `QuickJS` refuses.
    stopped: Cell<bool>,
}

impl EventState {
    fn new(names: Arc<SharedListenerNames>) -> Self {
        Self {
            listeners: RefCell::default(),
            by_node: RefCell::default(),
            names,
            stopped: Cell::default(),
        }
    }

    /// Records that `node` now has a listener for `(name, capture)`.
    fn enable(&self, node: dom::NodeId, name: &str, capture: bool) {
        let shared: Arc<str> = self
            .listeners
            .borrow()
            .get_key_value(name)
            .map_or_else(|| Arc::from(name), |(existing, _)| Arc::clone(existing));
        let fresh_registration = self
            .listeners
            .borrow_mut()
            .entry(Arc::clone(&shared))
            .or_default()
            .insert((node, capture));
        if fresh_registration {
            self.by_node
                .borrow_mut()
                .entry(node)
                .or_default()
                .push((shared, capture));
            self.names.note_enabled(name);
        }
    }

    /// The reverse: that registration went away.
    fn disable(&self, node: dom::NodeId, name: &str, capture: bool) {
        let mut listeners = self.listeners.borrow_mut();
        let Some(nodes) = listeners.get_mut(name) else {
            return;
        };
        if !nodes.remove(&(node, capture)) {
            return;
        }
        if nodes.is_empty() {
            listeners.remove(name);
        }
        drop(listeners);
        self.forget_node_listener(node, name, capture);
        self.names.note_disabled(name);
    }

    /// Drops every registration on an element that is going away.
    fn forget_node(&self, node: dom::NodeId) {
        let Some(registrations) = self.by_node.borrow_mut().remove(&node) else {
            return;
        };
        let mut listeners = self.listeners.borrow_mut();
        for (name, capture) in registrations {
            if let Some(nodes) = listeners.get_mut(&name) {
                nodes.remove(&(node, capture));
                if nodes.is_empty() {
                    listeners.remove(&name);
                }
            }
            // A drop is a removal like any other: the shared table must not
            // keep counting a registration the element took with it.
            self.names.note_disabled(&name);
        }
    }

    fn forget_node_listener(&self, node: dom::NodeId, name: &str, capture: bool) {
        let mut by_node = self.by_node.borrow_mut();
        let Some(registrations) = by_node.get_mut(&node) else {
            return;
        };
        registrations.retain(|(registered, pass)| registered.as_ref() != name || *pass != capture);
        if registrations.is_empty() {
            by_node.remove(&node);
        }
    }
}

/// The private main-thread runtime used by the engine pipeline.
pub(crate) struct MainThreadRuntime {
    engine: ScriptEngine,
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

impl MainThreadRuntime {
    pub(crate) fn new(
        document: LynxDocument,
        listener_names: Arc<SharedListenerNames>,
        hub: Arc<FrameHub>,
        wake: impl Fn() + 'static,
    ) -> Result<Self, MainThreadError> {
        let mut engine = ScriptEngine::new()
            .map_err(|error| MainThreadError::from_engine("creating the script realm", error))?;
        let events = Rc::new(EventState::new(listener_names));
        let tree = install_bobcat(&mut engine, document, hub, wake, &events)?;
        Ok(Self {
            engine,
            tree,
            events,
            next_event_id: 0,
        })
    }

    /// Commits and publishes when anything is stale. Called by the command
    /// loop at the end of every round.
    pub(crate) fn commit_if_dirty(&mut self) {
        self.tree.borrow_mut().commit_if_dirty();
    }

    /// Advances the animation timeline to the presenting side's clock
    /// reading. Whether anything changed is the next commit's business.
    pub(crate) fn begin_frame(&mut self, now: f64) {
        let _ = self.tree.borrow_mut().tree().advance_animations(now);
    }

    /// Drives the user-agent scroll chain the presenting side decided.
    pub(crate) fn apply_scroll(&mut self, from: dom::NodeId, delta: dom::Vector2D<f32>) {
        let mut handle = self.tree.borrow_mut();
        let document = handle.tree();
        if document.get(from).is_some() {
            let _ = document.scroll_chain(from, delta);
        }
    }

    /// Applies new device metrics.
    pub(crate) fn apply_resize(&mut self, width: f32, height: f32, device_pixel_ratio: f32) {
        let mut handle = self.tree.borrow_mut();
        let document = handle.tree();
        let viewport = document.viewport_size();
        if viewport.width.to_bits() != width.to_bits()
            || viewport.height.to_bits() != height.to_bits()
        {
            document.set_viewport(width, height);
        }
        if document.device_pixel_ratio().to_bits() != device_pixel_ratio.to_bits() {
            document.set_device_pixel_ratio(device_pixel_ratio);
        }
    }

    pub(crate) fn note_images_changed(&mut self) {
        self.tree.borrow_mut().tree().note_images_changed();
    }

    /// Runs `probe` against the owned document — the observation seam for
    /// everything outside this thread.
    pub(crate) fn with_document<R>(&mut self, probe: impl FnOnce(&mut LynxDocument) -> R) -> R {
        probe(self.tree.borrow_mut().tree())
    }

    /// Delivers one routed event the presenting side decided: the type and
    /// the target crossed as plain data; the propagation path is computed
    /// here, where the document is.
    ///
    /// A target freed since the decision formed resolves to nothing rather
    /// than a path — a `NodeId` names one node for the life of the document,
    /// so the check is one lookup and can never hit a stranger.
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
        target: dom::NodeId,
        name: &Arc<str>,
        detail_json: &Arc<str>,
    ) -> Result<bool, MainThreadError> {
        let steps = {
            let mut handle = self.tree.borrow_mut();
            let document = handle.tree();
            if document.get(target).is_none() {
                return Ok(false);
            }
            document.event_steps(target, true, true)
        };
        let steps = &steps;
        // One lookup for the whole walk, and the first thing done: an event no
        // listener registered for never reaches the realm and never takes the
        // document.
        //
        // The path is filtered against the index here, under the borrow,
        // rather than the index being copied out and consulted per step: a
        // listener may register or unregister from inside the walk, so the
        // borrow cannot be held across a call into the realm — but the
        // *path* is short and bounded, and the index is not.
        let mut deliverable: SmallVec<[(dom::NodeId, dom::NodeId, bool); INLINE_DELIVERIES]> =
            SmallVec::new();
        {
            let listeners = self.events.listeners.borrow();
            let Some(nodes) = listeners.get(name.as_ref()) else {
                return Ok(false);
            };
            deliverable.extend(
                steps
                    .steps()
                    .iter()
                    .filter(|step| nodes.contains(&(step.node, step.capture)))
                    .map(|step| (step.node, step.target, step.capture)),
            );
        }
        if deliverable.is_empty() {
            return Ok(false);
        }

        self.events.stopped.set(false);

        // Fresh per dispatch, and never reused by a live one: dispatch takes
        // `&mut self`, so a listener cannot start a second walk from inside
        // this one, and the realm drops its entry before this call returns.
        // The wrap is therefore unreachable rather than merely unlikely.
        let event_id = self.next_event_id;
        self.next_event_id = self.next_event_id.wrapping_add(1);

        let last = deliverable.len() - 1;
        let mut delivered = false;
        for (index, (node, target, capture)) in deliverable.into_iter().enumerate() {
            if self.events.stopped.get() {
                break;
            }
            let arguments = [
                HostArgument::Number(packed_node_id(node)),
                HostArgument::Number(packed_node_id(target)),
                HostArgument::Number(f64::from(u8::from(capture))),
                HostArgument::String(name),
                HostArgument::String(detail_json),
                HostArgument::Number(f64::from(event_id)),
                HostArgument::Boolean(index == last),
            ];
            let called = self.engine.call_module_export(
                ELEMENT_MODULE_SPECIFIER,
                EVENT_DISPATCH_EXPORT,
                &arguments,
            );
            if !called
                .map_err(|error| MainThreadError::from_engine("delivering an event", error))?
            {
                // The realm published no callback; nothing on this path will.
                break;
            }
            delivered = true;
        }
        // Listeners remove elements too; the count they ran up is settled
        // here, at the end of the walk, rather than per node.
        self.finish_batch(true)?;
        Ok(delivered)
    }

    pub(crate) fn run_main_thread_script(
        &mut self,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        let entry_source = format!("{ENTRY_PREAMBLE}{source}");
        self.engine
            .register_module_source(source_name, &entry_source)
            .map_err(|error| {
                MainThreadError::from_engine("registering the MTS entry module", error)
            })?;
        let entry_specifier = serde_json::to_string(source_name)
            .expect("serializing a Rust string as a JavaScript string cannot fail");
        let boot = format!(
            r#"import {{ lynx }} from "{RUNTIME_MODULE_SPECIFIER}";
import {{ __FlushElementTree }} from "{ELEMENT_MODULE_SPECIFIER}";

await import({entry_specifier});

let data = undefined;
if (typeof globalThis.processData === "function") {{
  data = globalThis.processData(data);
}}
if (typeof globalThis.renderPage === "function") {{
  globalThis.renderPage(data);
}} else {{
  lynx.getEngine().dispatchEvent({{ type: "__RenderPage", data }});
}}
__FlushElementTree();
"#
        );
        self.evaluate_module(&boot, BOOT_MODULE_SPECIFIER, "booting the MTS entry")
    }

    /// Runs a collection now. Dead handles are finalized inside it, so their
    /// `dropElement` calls reach the document before the batch it belongs to
    /// ends.
    ///
    /// The explicit entry point; production collection is paced by removals
    /// through [`Self::finish_batch`] instead.
    #[allow(
        dead_code,
        reason = "the explicit collection entry point, used by tests"
    )]
    pub(crate) fn collect_garbage(&mut self) -> Result<(), MainThreadError> {
        self.collect()
    }

    fn collect(&mut self) -> Result<(), MainThreadError> {
        self.tree.borrow_mut().removals = 0;
        self.engine
            .collect_garbage()
            .map_err(|error| MainThreadError::from_engine("collecting garbage", error))
    }

    /// Collects if enough held subtrees were removed since the last
    /// collection. Runs after every evaluation that succeeded; a failed one
    /// keeps its count for the next.
    fn finish_batch(&mut self, succeeded: bool) -> Result<(), MainThreadError> {
        let due = succeeded && self.tree.borrow_mut().take_collection_due();
        if due { self.collect() } else { Ok(()) }
    }

    pub(crate) fn evaluate_module(
        &mut self,
        source: &str,
        name: &str,
        phase: &'static str,
    ) -> Result<(), MainThreadError> {
        let result = self
            .engine
            .execute_module(source, name)
            .map_err(|error| MainThreadError::from_engine(phase, error));
        let finished = self.finish_batch(result.is_ok());
        result.and(finished)
    }
}

fn install_bobcat(
    engine: &mut ScriptEngine,
    document: LynxDocument,
    hub: Arc<FrameHub>,
    wake: impl Fn() + 'static,
    events: &Rc<EventState>,
) -> Result<Rc<RefCell<TreeHandle>>, MainThreadError> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        document,
        removals: 0,
        hub,
        wake: Box::new(wake),
    }));

    install_host_module(engine, &handle, events)?;
    install_event_members(engine, events)?;
    engine
        .register_module_source(RUNTIME_MODULE_SPECIFIER, RUNTIME_MODULE_SOURCE)
        .map_err(|error| {
            MainThreadError::from_engine("registering the Bobcat runtime module", error)
        })?;
    engine
        .register_module_source(ELEMENT_MODULE_SPECIFIER, ELEMENT_PAPI_SOURCE)
        .map_err(|error| {
            MainThreadError::from_engine("registering the Element PAPI module", error)
        })?;

    Ok(handle)
}

fn install(
    engine: &mut ScriptEngine,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostValue]) -> Result<HostValue, String> + 'static,
) -> Result<(), MainThreadError> {
    engine
        .register_host_module_function(HOST_MODULE_SPECIFIER, name, arity, Box::new(callback))
        .map_err(|error| MainThreadError::from_engine("installing the host module", error))
}

/// Installs native host-module exports that parse their arguments, borrow the
/// tree, and run against the private document. Each `$parser` is one of the
/// argument helpers below, applied at the argument's position; `NAME` is the
/// diagnostic prefix every helper and validator stitches into its error.
macro_rules! tree_members {
    ($engine:ident, $handle:ident; $(
        fn $name:ident($($arg:ident: $parser:ident),*) |$document:ident| $body:block
    )*) => {$({
        const NAME: &str = concat!("bobcat-internal:host.", stringify!($name));
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

fn install_host_module(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<TreeHandle>>,
    events: &Rc<EventState>,
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
        fn swapElement(a: node_id_argument, b: node_id_argument) |document| {
            validate_swap(document, NAME, a, b)?;
            document.swap_element(a, b);
            Ok(HostValue::Undefined)
        }
    }

    // The two removals are written out rather than generated, because each
    // also counts toward the next collection: a detached subtree is freed
    // only once the handles naming it are finalized.
    let tree = Rc::clone(handle);
    install(engine, "removeElement", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.removeElement";
        let child = node_id_argument(NAME, arguments, 0)?;
        let mut handle = borrow_tree(NAME, &tree)?;
        let document = handle.tree();
        validate_removable(document, NAME, child)?;
        document.remove_element(child);
        handle.note_removal();
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "replaceElement", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.replaceElement";
        let new_element = node_id_argument(NAME, arguments, 0)?;
        let old_element = node_id_argument(NAME, arguments, 1)?;
        let mut handle = borrow_tree(NAME, &tree)?;
        let document = handle.tree();
        validate_removable(document, NAME, old_element)?;
        validate_live_element(document, NAME, new_element)?;
        if let Some(parent) = document.get(old_element).and_then(dom::Node::parent_id) {
            validate_insert(document, NAME, parent, new_element, Some(old_element))?;
            document.insert_before(parent, new_element, Some(old_element));
            document.remove_element(old_element);
            handle.note_removal();
        }
        Ok(HostValue::Undefined)
    })?;

    install_attribute_members(engine, handle)?;

    let tree = Rc::clone(handle);
    let state = Rc::clone(events);
    // The realm's handle for `node` has been collected, and a handle is the
    // one thing that holds an element: the node is freed now. Only the node —
    // its element children are unlinked and go on as detached roots, each
    // held by the handle that names it, while the text node a `raw-text`
    // reflects goes with it because no handle could ever name one. Every
    // listener the realm had on it is gone too, since those lived on the
    // handle, so the index stops naming the node.
    install(engine, "dropElement", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.dropElement";
        let node = node_id_argument(NAME, arguments, 0)?;
        let mut tree = borrow_tree(NAME, &tree)?;
        let document = tree.tree();
        validate_removable(document, NAME, node)?;
        // A connected element's handle is held by its parent's, up to the
        // permanent page handle, so a connected element can never be the
        // subject of a drop; if one is, the graph and the tree disagree and
        // the realm must hear about it before the element is gone.
        if document.is_connected(node) {
            return Err(format!(
                "{NAME} was given a connected element: the element ownership \
                 graph and the tree disagree"
            ));
        }
        // Before the drop, so an id that somehow fails to free still leaves
        // the presenting thread's listener index naming nothing.
        state.forget_node(node);
        document.drop_element(node);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, "flushElementTree", 0, move |_arguments| {
        borrow_tree("bobcat-internal:host.flushElementTree", &tree)?.flush();
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// Installs the three members the realm's `EventTarget` speaks to.
///
/// None of them touches the document. The first two only maintain an index —
/// which nodes are worth visiting — and the third only sets a flag; see
/// [`EventState::stopped`].
fn install_event_members(
    engine: &mut ScriptEngine,
    events: &Rc<EventState>,
) -> Result<(), MainThreadError> {
    let state = Rc::clone(events);
    install(engine, "enableEventListener", 3, move |arguments| {
        let node = node_id_argument("bobcat-internal:host.enableEventListener", arguments, 0)?;
        let capture = capture_argument("bobcat-internal:host.enableEventListener", arguments, 1)?;
        let name = string_argument("bobcat-internal:host.enableEventListener", arguments, 2)?;
        state.enable(node, name, capture);
        Ok(HostValue::Undefined)
    })?;

    let state = Rc::clone(events);
    install(engine, "disableEventListener", 3, move |arguments| {
        let node = node_id_argument("bobcat-internal:host.disableEventListener", arguments, 0)?;
        let capture = capture_argument("bobcat-internal:host.disableEventListener", arguments, 1)?;
        let name = string_argument("bobcat-internal:host.disableEventListener", arguments, 2)?;
        state.disable(node, name, capture);
        Ok(HostValue::Undefined)
    })?;

    let state = Rc::clone(events);
    install(engine, "stopPropagation", 0, move |_arguments| {
        state.stopped.set(true);
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// Installs the DOM-attribute and inline-style portion of the host module.
///
/// `id`, `class`, and `style` deliberately travel through the same narrow
/// string boundary as every other attribute. [`LynxDocument`] owns their
/// specialized DOM/style invalidation paths. `setInlineStyles` is the one
/// wider primitive: a whole record in one crossing, because a record is a
/// whole-block replacement and building it from empty is what the setter
/// means. Nothing in the realm — the Element PAPI included — receives a
/// document handle.
fn install_attribute_members(
    engine: &mut ScriptEngine,
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
        fn setInlineStyles(node: node_id_argument, record: string_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let declarations = split_style_record(NAME, record)?;
            document.set_inline_style_declarations(node, declarations);
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
                .map(str::to_owned);
            Ok(value.map_or(HostValue::Null, HostValue::String))
        }
        fn tagName(node: node_id_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let tag = document
                .get(node)
                .and_then(dom::Node::tag_name)
                .ok_or_else(|| {
                    "bobcat-internal:host.tagName requires a live element tag".to_owned()
                })?;
            Ok(HostValue::String(tag.to_owned()))
        }
        fn attributeNames(node: node_id_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let mut record = String::new();
            if let Some(element) = document.get(node) {
                for (name, _) in element.attributes() {
                    write_record_field(&mut record, name);
                }
            }
            Ok(HostValue::String(record))
        }
        fn childElementIds(node: node_id_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let mut ids = String::new();
            if let Some(element) = document.get(node) {
                for child in element.children().filter(|child| child.is_element()) {
                    if !ids.is_empty() {
                        ids.push(',');
                    }
                    ids.push_str(&child.id().to_bits().to_string());
                }
            }
            Ok(HostValue::String(ids))
        }
    }

    Ok(())
}

/// Splits a `__SetInlineStyles` record payload into its declarations.
///
/// The payload is a flat sequence of `<units>:<text>` fields, two per
/// declaration — the hyphenated property name, then the value. `<units>` is
/// the text's length in UTF-16 code units, which is exactly what JavaScript's
/// `String.prototype.length` reports, so the writing side needs no scan and
/// no escaping.
///
/// Length-prefixing rather than delimiting is the point: a declaration value
/// is arbitrary author text, and any separator this could have used — a
/// semicolon, a NUL, a private-use code point — is a character some value may
/// legitimately contain. A length says where the next field starts without
/// asking what is inside this one.
fn split_style_record<'a>(
    function: &str,
    payload: &'a str,
) -> Result<SmallVec<[(&'a str, &'a str); INLINE_DECLARATIONS]>, String> {
    let mut declarations = SmallVec::new();
    let mut rest = payload;
    while !rest.is_empty() {
        let (property, after_property) = take_record_field(function, rest)?;
        let (value, after_value) = take_record_field(function, after_property)?;
        declarations.push((property, value));
        rest = after_value;
    }
    Ok(declarations)
}

/// Reads one `<units>:<text>` field, returning it and what follows.
fn take_record_field<'a>(function: &str, rest: &'a str) -> Result<(&'a str, &'a str), String> {
    let malformed = || format!("{function} received a malformed style record");
    let separator = rest.find(':').ok_or_else(malformed)?;
    let units: usize = rest[..separator].parse().map_err(|_| malformed())?;
    let body = &rest[separator + 1..];

    let mut counted = 0usize;
    let mut end = 0usize;
    for (offset, character) in body.char_indices() {
        if counted == units {
            end = offset;
            break;
        }
        counted += character.len_utf16();
        end = offset + character.len_utf8();
    }
    // Short of the count means the payload was truncated; past it means the
    // count landed inside a surrogate pair. Neither can come from the writer.
    if counted != units {
        return Err(malformed());
    }
    Ok((&body[..end], &body[end..]))
}

/// Appends one `<units>:<text>` field, [`take_record_field`]'s inverse; the
/// count is in UTF-16 code units because `String.prototype.slice` consumes it.
fn write_record_field(record: &mut String, text: &str) {
    let units: usize = text.chars().map(char::len_utf16).sum();
    record.push_str(&units.to_string());
    record.push(':');
    record.push_str(text);
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
/// handle safe: script holds these for as long as it likes, and a number it
/// stashed can outlive the element it named (a released element is freed once
/// it is detached), so such an id must resolve to nothing rather than to
/// whatever took its place.
#[allow(
    clippy::cast_precision_loss,
    reason = "a packed handle is built to stay inside f64's exact-integer range"
)]
fn packed_node_id(node: dom::NodeId) -> f64 {
    node.to_bits() as f64
}

/// The same handle as a value a host callback returns, rather than as an
/// argument the runtime lends into a call.
fn node_id_value(node: dom::NodeId) -> HostValue {
    HostValue::Number(packed_node_id(node))
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

#[cfg(test)]
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

    /// The name and detail a dispatch carries, spelled as the presenting side
    /// already owns them.
    fn tap() -> Arc<str> {
        Arc::from("tap")
    }

    fn no_detail() -> Arc<str> {
        Arc::from("")
    }

    fn runtime() -> (MainThreadRuntime, DocumentProbe) {
        runtime_over(new_document(
            Viewport::new(393.0, 727.0),
            PageConfig::default(),
        ))
    }

    /// The same runtime over a document that can shape text: Ahem's solid em
    /// squares make a run's box its glyph count times its font size.
    fn text_runtime() -> (MainThreadRuntime, DocumentProbe) {
        const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

        let mut document = new_document(Viewport::new(393.0, 727.0), PageConfig::default());
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        runtime_over(document)
    }

    fn runtime_over(document: LynxDocument) -> (MainThreadRuntime, DocumentProbe) {
        let (runtime, elements, _) = runtime_over_watching_names(document);
        (runtime, elements)
    }

    /// A same-thread window onto the runtime-owned document, so a test can
    /// observe what script built without going through the runtime's own
    /// methods.
    struct DocumentProbe(Rc<RefCell<TreeHandle>>);

    impl DocumentProbe {
        fn tree(&self) -> RefMut<'_, LynxDocument> {
            RefMut::map(self.0.borrow_mut(), TreeHandle::tree)
        }
    }

    /// The same runtime, plus the shared name table the presenting side
    /// filters against — so a test can ask what the realm published.
    fn runtime_over_watching_names(
        document: LynxDocument,
    ) -> (MainThreadRuntime, DocumentProbe, Arc<SharedListenerNames>) {
        let names = Arc::new(SharedListenerNames::default());
        let runtime = MainThreadRuntime::new(
            document,
            Arc::clone(&names),
            Arc::new(FrameHub::default()),
            || {},
        )
        .expect("main-thread runtime");
        let probe = DocumentProbe(Rc::clone(&runtime.tree));
        (runtime, probe, names)
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
    fn boot_dispatches_render_page_when_the_entry_has_no_global_function() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                const engine = lynx.getEngine();
                const page = __CreatePage('card', 0);
                globalThis.processData = function () {
                  return 42;
                };
                engine.addEventListener('__RenderPage', function (event) {
                  if (this !== engine || event.type !== '__RenderPage' || event.data !== 42) {
                    throw new Error('the engine render event lost its target or processed data');
                  }
                  __AppendElement(page, __CreateView(0));
                });
                ",
                "app:///engine-render.js",
            )
            .expect("engine render-event boot");

        assert!(elements.tree().get(node_id(3)).is_some());
    }

    #[test]
    fn boot_allows_an_entry_with_neither_render_path() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                "if ('renderPage' in globalThis) throw new Error('unexpected global');",
                "app:///no-render.js",
            )
            .expect("an entry is not required to assign renderPage or register a listener");

        assert!(
            elements.tree().document_element().child_ids().is_empty(),
            "an unhandled render event must leave the permanent page empty"
        );
    }

    #[test]
    fn boot_awaits_the_esm_entry_before_rendering_once() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                import { __CreateView as createView } from 'bobcat:element';
                await Promise.resolve();
                if (typeof globalThis.__CreateView !== 'undefined') {
                  throw new Error('Element PAPI must be ESM-only');
                }
                lynx.getEngine().addEventListener('__RenderPage', function () {
                  throw new Error('the fallback event must not accompany a global renderPage');
                });
                let renderCount = 0;
                globalThis.renderPage = function () {
                  renderCount += 1;
                  if (renderCount !== 1) {
                    throw new Error('renderPage ran more than once');
                  }
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, createView(0));
                };
                ",
                "app:///async-entry.mjs",
            )
            .expect("top-level-await entry boot");

        assert!(elements.tree().get(node_id(3)).is_some());
    }

    #[test]
    fn imported_runtime_bindings_supply_bridges_without_globals() {
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
                if (NativeModules !== undefined) {
                  throw new Error('the imported native-module sentinel must be undefined');
                }
                for (const name of [
                  'lynx', 'SystemInfo', '__globalProps', 'NativeModules',
                  '_AddEventListener', '_ReportError', '_SetSourceMapRelease',
                  '__OnLifecycleEvent', 'bobcat'
                ]) {
                  if (name in globalThis) {
                    throw new Error(name + ' must be supplied only by the injected import');
                  }
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
                "app:///runtime-imports.mjs",
            )
            .expect("imported runtime bindings");
    }

    #[test]
    fn get_engine_returns_one_event_target_with_standard_listener_identity() {
        let (mut runtime, _elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                const engine = lynx.getEngine();
                if (engine !== lynx.getEngine() ||
                    Object.prototype.toString.call(engine) !== '[object EventTarget]') {
                  throw new Error('getEngine must return one stable EventTarget');
                }
                const probe = { type: 'probe', data: 7 };
                const calls = [];
                function listener(event) {
                  if (this !== engine || event !== probe) {
                    throw new Error('function listeners need EventTarget receiver semantics');
                  }
                  calls.push('function');
                }
                const objectListener = {
                  handleEvent(event) {
                    if (this !== objectListener || event !== probe) {
                      throw new Error('listener objects need handleEvent receiver semantics');
                    }
                    calls.push('object');
                  }
                };
                engine.addEventListener('probe', listener);
                engine.addEventListener('probe', listener);
                engine.addEventListener('probe', listener, { capture: true, once: true });
                engine.addEventListener('probe', objectListener, { once: true });
                if (engine.dispatchEvent(probe) !== true ||
                    calls.join(',') !== 'function,function,object') {
                  throw new Error('engine listener identity or first dispatch is wrong: ' + calls);
                }
                calls.length = 0;
                engine.dispatchEvent(probe);
                if (calls.join(',') !== 'function') {
                  throw new Error('once listeners must leave only the persistent listener');
                }
                engine.removeEventListener('probe', listener);
                calls.length = 0;
                engine.dispatchEvent(probe);
                if (calls.length !== 0) {
                  throw new Error('removeEventListener must remove the matching listener');
                }
                ",
                "app:///engine-event-target.mjs",
            )
            .expect("engine EventTarget behavior");
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
                import { removeElement } from 'bobcat-internal:host';
                globalThis.renderPage = function () {
                  removeElement(999999);
                };
                ",
                "app:///invalid-tree-operation.js",
            )
            .expect_err("a stale id must be refused");

        assert!(error.source.message.contains("stale element id"));
        assert!(
            elements.tree().get(node_id(2)).is_some(),
            "a rejected callback leaves the document usable"
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
    fn record_inline_styles_are_resolved_by_name_before_reaching_stylo() {
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
    fn a_style_record_value_carries_delimiters_and_non_bmp_text_intact() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, {
                    '--separators': 'a:b 3:x 11:y',
                    '--astral': '\u{1F980}',
                    width: '7px',
                  });
                };
                ",
                "app:///delimiter-style.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        let style = elements
            .get(node_id(3))
            .expect("the view is live")
            .attribute("style")
            .expect("the record produced an inline style");
        assert!(style.contains("--separators: a:b 3:x 11:y"), "{style}");
        assert!(style.contains("--astral: \u{1F980}"), "{style}");
        assert!(style.contains("width: 7px"), "{style}");
    }

    /// A value the per-property setter would reject must stay rejected: a
    /// batch that concatenated the record into style-attribute text would let
    /// a `;` start a second declaration instead.
    #[test]
    fn a_style_record_value_cannot_inject_a_second_declaration() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, { width: '5px; height: 9px' });
                };
                ",
                "app:///injection-style.js",
            )
            .expect("main-thread script");

        let elements = elements.tree();
        let view = elements.get(node_id(3)).expect("the view is live");
        let style = view
            .attribute("style")
            .expect("an empty block is still set");
        assert!(!style.contains("height"), "{style}");
        assert!(!style.contains("width"), "{style}");
    }

    #[test]
    fn a_malformed_style_record_is_a_boundary_error_rather_than_a_guess() {
        for payload in ["4:ab", "notalength:x0:", "3:ab", "2:ab", "1:\u{1F980}x0:"] {
            assert!(
                split_style_record("bobcat.setInlineStyles", payload).is_err(),
                "{payload:?}"
            );
        }
    }

    #[test]
    fn a_style_record_splits_on_lengths_rather_than_delimiters() {
        let payload = "5:width4:10px11:font-family9:a;b:c 3:x";
        assert_eq!(
            split_style_record("bobcat.setInlineStyles", payload).expect("well-formed")[..],
            [("width", "10px"), ("font-family", "a;b:c 3:x")]
        );
        assert!(
            split_style_record("bobcat.setInlineStyles", "")
                .expect("an empty record")
                .is_empty()
        );
        assert_eq!(
            split_style_record("bobcat.setInlineStyles", "7:--empty0:").expect("empty value")[..],
            [("--empty", "")]
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

    /// The two indexes are one fact written twice, so every mutation has to
    /// leave them agreeing — including the shared name table the presenting
    /// side filters against.
    #[test]
    fn the_listener_indexes_and_the_published_names_stay_in_step() {
        let names = Arc::new(SharedListenerNames::default());
        let state = EventState::new(Arc::clone(&names));
        let (a, b) = (node_id(3), node_id(4));

        state.enable(a, "tap", false);
        state.enable(a, "tap", true);
        state.enable(a, "scroll", false);
        state.enable(b, "tap", false);
        assert!(names.contains("tap"));
        assert!(names.contains("scroll"));
        assert_eq!(state.by_node.borrow()[&a].len(), 3);
        assert_eq!(state.by_node.borrow()[&b].len(), 1);

        // A repeat registration is not a second one — neither index moves,
        // so the shared count does not either.
        state.enable(a, "tap", false);
        assert_eq!(state.by_node.borrow()[&a].len(), 3);

        state.disable(a, "scroll", false);
        assert!(!names.contains("scroll"));
        assert!(names.contains("tap"));
        assert_eq!(state.by_node.borrow()[&a].len(), 2);

        // Dropping an element takes its own registrations and only those.
        state.forget_node(a);
        assert!(!state.by_node.borrow().contains_key(&a));
        assert!(
            names.contains("tap"),
            "the sibling registration still holds the name open"
        );
        assert_eq!(
            state.listeners.borrow()["tap"]
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![(b, false)]
        );

        state.forget_node(b);
        assert!(state.listeners.borrow().is_empty());
        assert!(state.by_node.borrow().is_empty());
        assert!(
            !names.contains("tap"),
            "the last listener unpublishes its name"
        );
    }

    /// The shared name table is what the presenting side filters against, so
    /// a registration has to reach it as the realm makes it.
    #[test]
    fn registering_a_listener_publishes_its_name_to_the_shared_table() {
        let (mut runtime, _elements, names) = runtime_over_watching_names(new_document(
            Viewport::new(393.0, 727.0),
            PageConfig::default(),
        ));
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __AddEventListener(view, 'tap', () => {}, {});
                };
                ",
                "app:///publish.js",
            )
            .expect("main-thread script");
        assert!(names.contains("tap"));
        assert!(!names.contains("scroll"));

        // A second module rather than a second entry: the point is a later
        // unregistration, not a second boot.
        runtime
            .evaluate_module(
                r"
                import { __GetElementUniqueID } from 'bobcat:element';
                import { disableEventListener } from 'bobcat-internal:host';
                disableEventListener(__GetElementUniqueID(globalThis.held[1]), 0, 'tap');
                ",
                "app:///unpublish.mjs",
                "unpublishing",
            )
            .expect("unregistration");
        assert!(
            !names.contains("tap"),
            "the last listener for a name unpublishes it"
        );
    }

    #[test]
    fn a_dispatch_reaches_only_the_nodes_that_registered_a_listener() {
        let (mut runtime, _elements) = runtime();
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
            .dispatch_event(node_id(target), &tap(), &Arc::from("{\"x\":1}"))
            .expect("dispatch");
        assert!(delivered);

        runtime
            .evaluate_module(
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
        let (mut runtime, _elements) = runtime();
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
                .dispatch_event(node_id(4), &tap(), &no_detail())
                .expect("dispatch")
        );

        runtime
            .evaluate_module(
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
        let (mut runtime, _elements) = runtime();
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
                .dispatch_event(node_id(3), &tap(), &no_detail())
                .expect("dispatch")
        );

        runtime
            .evaluate_module(
                r"
                import { __AddEvent, __GetEvent } from 'bobcat:element';
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
                .dispatch_event(node_id(3), &tap(), &no_detail())
                .expect("dispatch")
        );
    }

    /// The id and the last-call flag are the two things a delivery carries
    /// beyond the path itself, and both are observable from the realm: the id
    /// is what makes one walk hold one event object, and the flag is what ends
    /// the dispatch — which the standard makes visible by resetting
    /// `eventPhase` and `currentTarget` on an event a listener kept.
    #[test]
    fn one_id_names_a_whole_walk_and_only_its_last_delivery_is_flagged() {
        let (mut runtime, _elements) = runtime();
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
                  globalThis.held = [page, outer, inner];
                  const record = (where) => (event) => {
                    seen.push({ where, event, phase: event.eventPhase });
                  };
                  __AddEventListener(page, 'tap', record('page'), { capture: true });
                  __AddEventListener(inner, 'tap', record('inner'), {});
                };
                ",
                "app:///listeners.js",
            )
            .expect("main-thread script");

        for _ in 0..2 {
            assert!(
                runtime
                    .dispatch_event(node_id(4), &tap(), &no_detail())
                    .expect("dispatch")
            );
        }

        runtime
            .evaluate_module(
                r"
                // Two deliveries per walk: `outer` registered nothing, so it
                // is on the path but never reached.
                const order = seen.map((step) => step.where).join('|');
                if (order !== 'page|inner|page|inner') {
                  throw new Error('deliveries: ' + order);
                }
                // One id, one event object — which is what lets a property one
                // listener writes reach the next.
                if (seen[0].event !== seen[1].event || seen[2].event !== seen[3].event) {
                  throw new Error('a walk minted more than one event');
                }
                if (seen[0].event === seen[2].event) {
                  throw new Error('two walks shared one event');
                }
                // Read while the dispatch was live: capturing at the ancestor,
                // at-target on the target itself.
                const phases = seen.map((step) => step.phase).join('|');
                if (phases !== '1|2|1|2') {
                  throw new Error('phases: ' + phases);
                }
                // The last delivery of each walk was flagged, so the realm
                // ended the dispatch rather than leaving the kept event still
                // naming whichever node it stopped on.
                for (const { event } of seen) {
                  if (event.eventPhase !== 0 || event.currentTarget !== null) {
                    throw new Error('a dispatch outlived its walk');
                  }
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
            .dispatch_event(node_id(3), &tap(), &no_detail())
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
        let (mut runtime, _elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  // Detached and let go of: this is the element a sweep
                  // collects. Attached, the page's handle would keep it.
                  const doomed = __CreateView(0);
                  __AppendElement(page, doomed);
                  __RemoveElement(page, doomed);
                  globalThis.doomed = __GetElementUniqueID(doomed);
                  globalThis.held = [page, view];
                  __AddEventListener(page, 'tap', () => seen.push('page'), { capture: true });
                  __AddEventListener(view, 'tap', () => seen.push('view'), {});
                };
                ",
                "app:///collect.js",
            )
            .expect("main-thread script");

        // Collect the unrelated handle between building the path and running
        // the walk. The real finalizer performs the one `dropElement` call;
        // invoking it manually here would leave that finalizer armed and make
        // its later cleanup a duplicate stale-id call.
        runtime.collect_garbage().expect("sweep");

        runtime
            .dispatch_event(node_id(3), &tap(), &no_detail())
            .expect("dispatch");

        // A collected handle is routine — a ReactLynx re-render drops them
        // constantly — so it must not silently cost the rest of the walk.
        runtime
            .evaluate_module(
                "if (seen.join('|') !== 'page|view') throw new Error('truncated: ' + seen.join('|'));",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");
    }

    #[test]
    fn stopping_propagation_ends_the_walk() {
        let (mut runtime, _elements) = runtime();
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
            .dispatch_event(node_id(3), &tap(), &no_detail())
            .expect("dispatch");

        runtime
            .evaluate_module(
                "if (seen.join('|') !== 'page') throw new Error('got ' + seen.join('|'));",
                "app:///verify.js",
                "verifying",
            )
            .expect("verification");
    }

    #[test]
    fn a_document_whose_script_registered_nothing_never_enters_the_realm() {
        let (mut runtime, _elements) = runtime();
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
                .dispatch_event(node_id(3), &tap(), &no_detail())
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

    /// A handle is what keeps an element alive, and while the element is
    /// attached its handle is kept by its parent's — up to the permanent page
    /// handle. So a `ReactLynx` list handing a recycled cell's elements
    /// between snapshot instances, and deleting the old `__elements` array,
    /// takes nothing away: the elements are on screen and their handles are
    /// reachable from the page. What ends a subtree is detaching it and then
    /// letting go.
    #[test]
    fn an_attached_element_s_handle_is_kept_by_its_parent_s() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const wrapper = __CreateView(0);
                  __AppendElement(page, wrapper);
                  let child = __CreateView(0);
                  __AppendElement(wrapper, child);
                  // The snapshot instance that created it lets go; the
                  // wrapper's handle is the one that holds it now.
                  child = undefined;
                  globalThis.wrapper = wrapper;
                };
                ",
                "app:///attached.js",
            )
            .expect("main-thread script");

        runtime.collect_garbage().expect("collection");
        assert_eq!(
            elements
                .tree()
                .get(node_id(4))
                .and_then(dom::Node::parent_id),
            Some(node_id(3)),
            "the element script let go of is still attached under its parent"
        );

        runtime
            .evaluate_module(
                "import { __CreatePage, __RemoveElement } from 'bobcat:element';
                 __RemoveElement(__CreatePage('card', 0), globalThis.wrapper);",
                "app:///detach.js",
                "detaching",
            )
            .expect("detach");
        let tree = elements.tree();
        assert!(
            tree.get(node_id(4)).is_some(),
            "a removal frees nothing: the wrapper's handle still names both"
        );
        drop(tree);

        runtime
            .evaluate_module(
                "globalThis.wrapper = undefined;",
                "app:///let-go.js",
                "letting go",
            )
            .expect("let go");
        runtime.collect_garbage().expect("collection");
        let tree = elements.tree();
        assert!(
            tree.get(node_id(3)).is_none(),
            "the detached wrapper goes once its handle does"
        );
        assert!(
            tree.get(node_id(4)).is_none(),
            "and the child with it: the wrapper's handle held the only \
             reference left to the child's"
        );
    }

    /// The whole ownership graph, through every mutation that changes a
    /// parent. Script keeps no reference of its own to anything, so the only
    /// thing that can survive a collection is what the page's permanent
    /// handle holds through the chain of child sets — which must be exactly
    /// the connected elements, and nothing more.
    #[test]
    fn every_connected_element_survives_a_collection_script_holds_nothing_through() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const a = __CreateView(0);
                  __AppendElement(page, a);
                  const b = __CreateView(0);
                  __InsertElementBefore(page, b, a);
                  // A move: b leaves the page's set for a's.
                  __InsertElementBefore(a, b, null);
                  const c = __CreateView(0);
                  __ReplaceElement(c, b);
                  const d = __CreateView(0);
                  const e = __CreateView(0);
                  __ReplaceElements(a, [d, e], [c]);
                  // Within one parent, then across two.
                  __SwapElement(d, e);
                  const f = __CreateView(0);
                  __AppendElement(page, f);
                  __SwapElement(d, f);
                  const g = __CreateView(0);
                  __AppendElement(page, g);
                  __RemoveElement(page, g);
                };
                ",
                "app:///ownership.js",
            )
            .expect("main-thread script");

        runtime.collect_garbage().expect("collection");
        let tree = elements.tree();
        // page 2, a 3, b 4, c 5, d 6, e 7, f 8, g 9.
        for (id, parent) in [(3, 2), (6, 2), (7, 3), (8, 3)] {
            assert_eq!(
                tree.get(node_id(id)).and_then(dom::Node::parent_id),
                Some(node_id(parent)),
                "node {id} is connected, so its handle is reachable from the page's"
            );
        }
        for id in [4, 5, 9] {
            assert!(
                tree.get(node_id(id)).is_none(),
                "node {id} was left detached and unreferenced, so its handle went"
            );
        }
    }

    /// The invariant, checked rather than argued: a connected element's
    /// handle is held by its parent's, so a drop can never name one. If it
    /// does, the realm's ownership graph has diverged from the tree, and the
    /// element must not quietly disappear from the screen.
    #[test]
    fn dropping_a_connected_element_is_refused() {
        let (mut runtime, elements) = runtime();
        let error = runtime
            .run_main_thread_script(
                r"
                import { dropElement } from 'bobcat-internal:host';
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  dropElement(__GetElementUniqueID(view));
                };
                ",
                "app:///connected-drop.js",
            )
            .expect_err("a connected element cannot be dropped");
        assert!(error.to_string().contains("ownership graph"), "{error}");
        assert!(
            elements.tree().get(node_id(3)).is_some(),
            "and the element is still there"
        );
    }

    /// A handle that script has let go of reads as gone at once — `QuickJS`
    /// answers a `WeakRef` from the refcount — while its element stays
    /// allocated and stays a parent until the collection that finalizes it.
    /// So the ownership graph must never be what decides which native
    /// operation runs: a child of a let-go parent is still attached, and
    /// treating it as detached turns this swap into a silent deletion of the
    /// element it was swapped with.
    #[test]
    fn a_child_of_a_let_go_parent_is_still_attached_for_the_host() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const visible = __CreateView(0);
                  __AppendElement(page, visible);
                  globalThis.cell = (function () {
                    const wrapper = __CreateWrapperElement(0);
                    const cell = __CreateView(0);
                    __AppendElement(wrapper, cell);
                    // The wrapper's handle is unreachable from here on, and
                    // no collection has run: its element is still `cell`'s
                    // parent.
                    return cell;
                  })();
                  __SwapElement(globalThis.cell, visible);
                };
                ",
                "app:///let-go-parent.js",
            )
            .expect("main-thread script");

        // page 2, visible 3, wrapper 4, cell 5.
        let tree = elements.tree();
        assert_eq!(
            tree.get(node_id(5)).and_then(dom::Node::parent_id),
            Some(node_id(2)),
            "the swap moved the cell under the page"
        );
        assert_eq!(
            tree.get(node_id(3)).and_then(dom::Node::parent_id),
            Some(node_id(4)),
            "and moved the visible element under the wrapper, rather than \
             deleting it as a replace would have"
        );
    }

    /// A drop frees one node. A descendant script still names is unlinked
    /// from the freed ancestor and goes on as a detached root it can attach
    /// somewhere else — the ancestor's handle dying does not take it.
    #[test]
    fn dropping_a_detached_ancestor_leaves_a_still_named_descendant_a_root() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  let outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  __RemoveElement(page, outer);
                  outer = undefined;
                  globalThis.inner = inner;
                };
                ",
                "app:///ancestor.js",
            )
            .expect("main-thread script");

        runtime.collect_garbage().expect("collection");
        let tree = elements.tree();
        assert!(
            tree.get(node_id(3)).is_none(),
            "the detached ancestor is freed with its handle"
        );
        let inner = tree
            .get(node_id(4))
            .expect("the descendant script still names stays allocated");
        assert_eq!(inner.parent_id(), None, "as a detached root of its own");
        drop(tree);

        runtime
            .evaluate_module(
                "import { __AppendElement, __CreatePage } from 'bobcat:element';
                 __AppendElement(__CreatePage('card', 0), globalThis.inner);",
                "app:///reattach.js",
                "re-attaching",
            )
            .expect("the surviving handle still works");
        assert_eq!(
            elements
                .tree()
                .get(node_id(4))
                .and_then(dom::Node::parent_id),
            Some(node_id(2))
        );
    }

    /// `ReactLynx`'s unmount: `__RemoveElement` on the snapshot's root, then
    /// every handle of the subtree is let go at once. Whatever order the
    /// finalizer delivers those in, the whole subtree is gone after one
    /// collection and the ids are retired.
    #[test]
    fn an_unmounted_subtree_is_freed_by_the_collection_that_takes_its_handles() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const root = __CreateView(0);
                  const middle = __CreateText(0);
                  const leaf = __CreateRawText('leaf');
                  __AppendElement(page, root);
                  __AppendElement(root, middle);
                  __AppendElement(middle, leaf);
                  __RemoveElement(page, root);
                  // `__elements` of the unmounted snapshot instance, deleted.
                };
                ",
                "app:///unmount.js",
            )
            .expect("main-thread script");
        assert_eq!(
            elements
                .tree()
                .get(node_id(5))
                .and_then(dom::Node::parent_id),
            Some(node_id(4)),
            "before collection the detached subtree is intact"
        );

        runtime.collect_garbage().expect("collection");
        let tree = elements.tree();
        for id in 3..=6 {
            assert!(
                tree.get(node_id(id)).is_none(),
                "node {id} of the unmounted subtree (incl. the raw-text run) is freed"
            );
        }
    }

    /// `__ReplaceElement` detaches what it replaces, and the detached
    /// element is kept by the handle script still holds — together with the
    /// subtree under it, whose handles that one holds in turn. Both go when
    /// script lets go.
    #[test]
    fn a_replaced_element_lives_as_long_as_the_handle_that_names_it() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const holder = __CreateView(0);
                  let inner = __CreateView(0);
                  __AppendElement(page, holder);
                  __AppendElement(holder, inner);
                  inner = undefined;
                  globalThis.holder = holder;
                };
                ",
                "app:///removal.js",
            )
            .expect("main-thread script");
        runtime.collect_garbage().expect("collection");
        assert!(elements.tree().get(node_id(4)).is_some());

        runtime
            .evaluate_module(
                "import { __CreateView, __ReplaceElement } from 'bobcat:element';
                 __ReplaceElement(__CreateView(0), globalThis.holder);",
                "app:///replace.js",
                "replacing",
            )
            .expect("replace");
        let tree = elements.tree();
        assert_eq!(
            tree.get(node_id(3)).and_then(dom::Node::parent_id),
            None,
            "the replaced holder is detached, and live: its handle names it"
        );
        assert!(
            tree.get(node_id(4)).is_some(),
            "and it holds the handle of the child under it"
        );
        drop(tree);

        runtime
            .evaluate_module(
                "import { __RemoveElement } from 'bobcat:element';
                 __RemoveElement(null, globalThis.holder);",
                "app:///noop.js",
                "no-op",
            )
            .expect("removing a detached element is a no-op");
        runtime
            .evaluate_module(
                "globalThis.holder = undefined;",
                "app:///let-go.js",
                "letting go",
            )
            .expect("let go");
        runtime.collect_garbage().expect("collection");
        let tree = elements.tree();
        assert!(tree.get(node_id(3)).is_none() && tree.get(node_id(4)).is_none());
    }

    /// A drop is immediate and final: the element is gone the moment the
    /// finalizer's call lands, and the id it used names nothing afterwards.
    #[test]
    fn a_drop_frees_the_element_at_once_and_retires_its_id() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                import { dropElement, tagName } from 'bobcat-internal:host';
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const gone = __CreateView(0);
                  __AppendElement(page, gone);
                  __RemoveElement(page, gone);
                  globalThis.goneId = __GetElementUniqueID(gone);
                  if (tagName(goneId) !== 'view') {
                    throw new Error('the detached element is gone before its drop');
                  }
                  // Called directly, where a finalizer would.
                  dropElement(goneId);
                };
                ",
                "app:///drop.js",
            )
            .expect("main-thread script");
        assert!(elements.tree().get(node_id(3)).is_none());
        runtime
            .evaluate_module(
                "import { tagName } from 'bobcat-internal:host';
                 tagName(globalThis.goneId);",
                "app:///after.js",
                "reading a freed id",
            )
            .expect_err("a freed id names nothing");
    }

    /// A listener that captures its own element must not keep it alive: the
    /// closure is reachable only from the handle it captures, so the cycle
    /// has no root once script lets go. This is exactly what a per-handle
    /// store in a `WeakMap` would break under `QuickJS`, whose `WeakMap` marks
    /// its values unconditionally.
    #[test]
    fn a_listener_capturing_its_own_element_does_not_keep_it_alive() {
        for (label, registration) in [
            (
                "listener closure",
                "{ const self = view; __AddEventListener(view, 'tap', () => self, {}); }",
            ),
            (
                "worklet handler",
                "__AddEvent(view, 'bindEvent', 'tap', { type: 'worklet', value: { ref: view } });",
            ),
            (
                "list callbacks",
                "{ const self = view; __UpdateListCallbacks(view, () => self, () => self, () => self); }",
            ),
        ] {
            let (mut runtime, elements) = runtime();
            runtime
                .run_main_thread_script(
                    &format!(
                        r"
                    globalThis.renderPage = function () {{
                      const page = __CreatePage('card', 0);
                      let view = __CreateView(0);
                      __AppendElement(page, view);
                      {registration}
                      __RemoveElement(page, view);
                      view = undefined;
                    }};
                    "
                    ),
                    "app:///self-capture.js",
                )
                .expect("main-thread script");
            runtime.collect_garbage().expect("collection");
            runtime.collect_garbage().expect("collection");
            let tree = elements.tree();
            assert!(
                tree.get(node_id(3)).is_none(),
                "{label}: the element whose handle only its own registration reached is freed"
            );
        }
    }

    /// Removals pace collection: once enough subtrees have been removed,
    /// the batch that crosses the count ends with a collection, so the
    /// handles those subtrees left behind are finalized and the subtrees freed
    /// without any allocation pressure or explicit collection.
    #[test]
    fn enough_removals_end_a_batch_with_a_collection() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  globalThis.page = page;
                  globalThis.churn = function (count) {
                    for (let i = 0; i < count; i += 1) {
                      const cell = __CreateView(0);
                      __AppendElement(page, cell);
                      __RemoveElement(page, cell);
                    }
                  };
                };
                ",
                "app:///paced.js",
            )
            .expect("main-thread script");

        let below = REMOVALS_PER_COLLECTION - 1;
        runtime
            .evaluate_module(
                &format!("globalThis.churn({below});"),
                "app:///below.js",
                "churning",
            )
            .expect("churn");
        // The count, not the tree, is the witness: QuickJS may collect on its
        // own allocation pressure at any point, which frees cells too, but
        // only the paced collection resets the count.
        assert_eq!(
            runtime.tree.borrow().removals,
            below,
            "below the count, no paced collection has run"
        );

        runtime
            .evaluate_module("globalThis.churn(1);", "app:///cross.js", "churning")
            .expect("churn");
        assert_eq!(
            runtime.tree.borrow().removals,
            0,
            "crossing the count ran the collection and reset it"
        );
        let tree = elements.tree();
        for id in 3..3 + u64::from(REMOVALS_PER_COLLECTION) {
            assert!(
                tree.get(node_id(id)).is_none(),
                "cell {id}: the batch that crossed the count collected and freed it"
            );
        }
    }

    /// Every element on an event path carries a handle — a connected one is
    /// held by its parent's, up to the permanent page handle — so a target
    /// always resolves to one. A target that does not is the ownership graph
    /// and the tree disagreeing, and the realm says so instead of inventing
    /// an `Event` that cannot name what it happened to.
    ///
    /// Routing cannot produce one today: it targets elements, and a hit on a
    /// text run maps to its element in `hit.rs`. This builds the path by hand
    /// against the run itself, the one node no handle ever names. The case
    /// that *will* produce one is a UA component with hit-testable shadow
    /// chrome — `first_element_at` answers with the flat-tree element it
    /// hits, shadow tree included, and script names no shadow node — so the
    /// first such component owes the event path a retarget to its host, the
    /// same one `event_path` already performs for every step outside the
    /// tree. `raw-text`, the only component today, has no shadow root.
    #[test]
    fn an_event_target_no_handle_names_is_an_error_not_a_silent_drop() {
        let (mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __AppendElement(page, text);
                  __AppendElement(text, __CreateRawText('hello'));
                  __AddEventListener(text, 'tap', () => {}, {});
                };
                ",
                "app:///run-target.js",
            )
            .expect("main-thread script");
        // page 2, text 3, raw-text 4, and the run the component reflects, 5.
        assert!(
            elements
                .tree()
                .get(node_id(5))
                .is_some_and(|node| !node.is_element()),
            "the run is the node the realm mints no handle for"
        );

        let error = runtime
            .dispatch_event(node_id(5), &tap(), &no_detail())
            .expect_err("a target no handle names cannot be delivered");
        assert!(error.to_string().contains("ownership graph"), "{error}");
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
