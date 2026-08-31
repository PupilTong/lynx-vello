//! The Lynx main-thread runtime over its owned `QuickJS` realm.

use std::cell::{Cell, RefCell, RefMut};
use std::fmt::{self, Write as _};
use std::rc::Rc;
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;

use super::ToPresenterSender;
use super::quickjs::ScriptEngine;
use crate::main::tree::LynxDocument;
use crate::script::ScriptError;
use crate::view::{EventRequester, ToPresenter};

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
    include_str!("../../../../../packages/bobcat-element/src/element-papi.mjs");
const RUNTIME_MODULE_SOURCE: &str =
    include_str!("../../../../../packages/bobcat-element/src/main-thread-runtime.mjs");

const ENTRY_PREAMBLE: &str = r#"import {
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

pub(crate) fn entry_module_source(source: &str) -> String {
    let mut module = String::with_capacity(ENTRY_PREAMBLE.len() + source.len());
    module.push_str(ENTRY_PREAMBLE);
    module.push_str(source);
    module
}

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
struct TreeHandle<R: EventRequester> {
    document: LynxDocument,
    /// Removals since the last collection; see [`REMOVALS_PER_COLLECTION`].
    removals: u32,
    /// Where committed frames leave for the presenting side.
    notify: ToPresenterSender<R>,
}

impl<R: EventRequester> TreeHandle<R> {
    /// Runs the whole pipeline and publishes the committed frame — the
    /// native half of `__FlushElementTree`, and the only place frames leave
    /// this thread.
    fn flush(&mut self) {
        self.notify.publish_frame(self.document.commit());
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
type ListenerNodes = FxHashSet<(dom::NodeId, bool)>;

/// The `(name, is capture pass)` pairs one node carries listeners for.
type NodeListeners = SmallVec<[(Arc<str>, bool); INLINE_NODE_LISTENERS]>;

/// What the realm has told the host about listeners, and what it tells it
/// during a walk.
///
/// Shared with the host functions that maintain it, so it is `Rc` rather than
/// owned: the native `enableEventListener` export and the dispatch driver are
/// different stack frames on the same thread.
struct EventState<R: EventRequester> {
    /// The nodes the realm has a listener on, per event name and pass. Keyed
    /// by name first so a walk resolves it once and then tests each step
    /// without touching the name again — and so an event no listener wants
    /// costs one lookup for the whole walk.
    listeners: RefCell<FxHashMap<Arc<str>, ListenerNodes>>,
    /// The same registrations keyed the other way, so dropping an element
    /// costs its own listeners rather than a scan of every name.
    by_node: RefCell<FxHashMap<dom::NodeId, NodeListeners>>,
    /// Where the presenting thread's replica of the name set is fed from.
    ///
    /// Sent from here rather than at a batch boundary because this is where
    /// the realm has just been told, and only on a global edge of
    /// `listeners`: the first registration for a name and the removal of its
    /// last. A second listener for a name already open sends nothing, so the
    /// traffic is registration edges, never registrations. Every send happens
    /// after the index it announces has been updated and its borrow released,
    /// so the truth is never behind what has crossed, and no `RefCell` is
    /// held across one.
    notify: ToPresenterSender<R>,
    /// Set by the native `stopPropagation` export. A pure flag write: the
    /// realm is inside a `call_module_export` when it runs, and re-entering
    /// the realm from a host function would nest an execution guard, which
    /// `QuickJS` refuses.
    stopped: Cell<bool>,
}

impl<R: EventRequester> EventState<R> {
    fn new(notify: ToPresenterSender<R>) -> Self {
        Self {
            listeners: RefCell::default(),
            by_node: RefCell::default(),
            notify,
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
        let mut listeners = self.listeners.borrow_mut();
        let nodes = listeners.entry(Arc::clone(&shared)).or_default();
        // No name is ever left keyed to an empty set, so an empty one here is
        // the entry `or_default` just made: this is the name's first listener
        // anywhere in the document.
        let first_for_name = nodes.is_empty();
        let fresh_registration = nodes.insert((node, capture));
        drop(listeners);
        if fresh_registration {
            self.by_node
                .borrow_mut()
                .entry(node)
                .or_default()
                .push((Arc::clone(&shared), capture));
            if first_for_name {
                self.notify.send(ToPresenter::ListenerAvailable(shared));
            }
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
        // The key comes back out with the removal: it is the `Arc` every
        // index already shares, so publishing the edge allocates nothing.
        let closed = if nodes.is_empty() {
            listeners.remove_entry(name).map(|(name, _)| name)
        } else {
            None
        };
        drop(listeners);
        self.forget_node_listener(node, name, capture);
        if let Some(name) = closed {
            self.notify.send(ToPresenter::ListenerUnavailable(name));
        }
    }

    /// Drops every registration on an element that is going away.
    fn forget_node(&self, node: dom::NodeId) {
        let Some(registrations) = self.by_node.borrow_mut().remove(&node) else {
            return;
        };
        let mut closed = SmallVec::<[Arc<str>; INLINE_NODE_LISTENERS]>::new();
        let mut listeners = self.listeners.borrow_mut();
        for (name, capture) in registrations {
            if let Some(nodes) = listeners.get_mut(&name)
                && nodes.remove(&(node, capture))
                && nodes.is_empty()
            {
                // A drop is a removal like any other: an element that took
                // the last listener for a name with it closes that name.
                listeners.remove(&name);
                closed.push(name);
            }
        }
        drop(listeners);
        for name in closed {
            self.notify.send(ToPresenter::ListenerUnavailable(name));
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
pub(crate) struct MainThreadRuntime<R: EventRequester> {
    engine: ScriptEngine,
    tree: Rc<RefCell<TreeHandle<R>>>,
    events: Rc<EventState<R>>,
    /// Names one dispatch, so the realm can keep one event object alive across
    /// the whole walk instead of minting one per node. Not shared with the
    /// host functions: only [`Self::dispatch_event`] reads or advances it, and
    /// it holds `&mut self` while it does.
    next_event_id: u32,
}

impl<R: EventRequester> fmt::Debug for MainThreadRuntime<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadRuntime")
            .finish_non_exhaustive()
    }
}

impl<R: EventRequester> MainThreadRuntime<R> {
    pub(crate) fn new(
        document: LynxDocument,
        notify: ToPresenterSender<R>,
    ) -> Result<Self, MainThreadError> {
        let mut engine = ScriptEngine::new()
            .map_err(|error| MainThreadError::from_engine("creating the script realm", error))?;
        let events = Rc::new(EventState::new(notify.clone()));
        let tree = install_bobcat(&mut engine, document, notify, &events)?;
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
        let _ = self.tree.borrow_mut().document.advance_animations(now);
    }

    /// Writes the presenting side's scroll offsets into the document and
    /// repaints: the commit at the end of this round bakes windows
    /// re-centered on them. This is the only way a user scroll reaches the
    /// document — between refills the offsets live on the presenting side
    /// alone.
    pub(crate) fn refill_scroll_windows(&mut self, offsets: &[(dom::NodeId, dom::Vector2D<f32>)]) {
        let mut handle = self.tree.borrow_mut();
        let document = &mut handle.document;
        for (node, offset) in offsets {
            document.scroll_to(*node, *offset);
        }
        document.note_scroll_windows_stale();
    }

    /// Applies new device metrics.
    pub(crate) fn apply_resize(&mut self, width: f32, height: f32, device_pixel_ratio: f32) {
        let mut handle = self.tree.borrow_mut();
        let document = &mut handle.document;
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
        self.tree.borrow_mut().document.note_images_changed();
    }

    /// Runs `probe` against the owned document — the observation seam for
    /// everything outside this thread.
    pub(crate) fn with_document<T>(&mut self, probe: impl FnOnce(&mut LynxDocument) -> T) -> T {
        probe(&mut self.tree.borrow_mut().document)
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
        name: &str,
        detail_json: &str,
    ) -> Result<bool, MainThreadError> {
        // Reject an event nobody listens for before computing its DOM path.
        let listeners = self.events.listeners.borrow();
        let Some(nodes) = listeners.get(name) else {
            return Ok(false);
        };
        let steps = {
            let handle = self.tree.borrow();
            let document = &handle.document;
            if document.get(target).is_none() {
                return Ok(false);
            }
            document.event_steps(target, true, true)
        };
        let mut deliverable: SmallVec<[(dom::NodeId, dom::NodeId, bool); INLINE_DELIVERIES]> =
            SmallVec::new();
        deliverable.extend(
            steps
                .steps()
                .iter()
                .filter(|step| nodes.contains(&(step.node, step.capture)))
                .map(|step| (step.node, step.target, step.capture)),
        );
        drop(listeners);
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
        let entry_source = entry_module_source(source);
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

    fn collect_garbage(&mut self) -> Result<(), MainThreadError> {
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
        if due { self.collect_garbage() } else { Ok(()) }
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

fn install_bobcat<R: EventRequester>(
    engine: &mut ScriptEngine,
    document: LynxDocument,
    notify: ToPresenterSender<R>,
    events: &Rc<EventState<R>>,
) -> Result<Rc<RefCell<TreeHandle<R>>>, MainThreadError> {
    let handle = Rc::new(RefCell::new(TreeHandle {
        document,
        removals: 0,
        notify,
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
            let $document = &mut handle.document;
            $body
        })?;
    })*};
}

fn install_host_module<R: EventRequester>(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<TreeHandle<R>>>,
    events: &Rc<EventState<R>>,
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
        let document = &mut handle.document;
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
        let document = &mut handle.document;
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
        let document = &mut tree.document;
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
fn install_event_members<R: EventRequester>(
    engine: &mut ScriptEngine,
    events: &Rc<EventState<R>>,
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
fn install_attribute_members<R: EventRequester>(
    engine: &mut ScriptEngine,
    handle: &Rc<RefCell<TreeHandle<R>>>,
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
            let element = validate_live_element(document, NAME, node)?;
            let value = element.attribute(name).map(str::to_owned);
            Ok(value.map_or(HostValue::Null, HostValue::String))
        }
        fn tagName(node: node_id_argument) |document| {
            let tag = validate_live_element(document, NAME, node)?
                .tag_name()
                .ok_or_else(|| {
                    "bobcat-internal:host.tagName requires a live element tag".to_owned()
                })?;
            Ok(HostValue::String(tag.to_owned()))
        }
        fn attributeNames(node: node_id_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let mut record = String::new();
            for (name, _) in element.attributes() {
                write_record_field(&mut record, name);
            }
            Ok(HostValue::String(record))
        }
        fn childElementIds(node: node_id_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let mut ids = String::new();
            for child in element.children().filter(|child| child.is_element()) {
                if !ids.is_empty() {
                    ids.push(',');
                }
                write!(&mut ids, "{}", child.id().to_bits())
                    .expect("writing to a String cannot fail");
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
    write!(record, "{units}:").expect("writing to a String cannot fail");
    record.push_str(text);
}

fn borrow_tree<'a, R: EventRequester>(
    function: &str,
    tree: &'a Rc<RefCell<TreeHandle<R>>>,
) -> Result<RefMut<'a, TreeHandle<R>>, String> {
    tree.try_borrow_mut()
        .map_err(|_| format!("{function} cannot re-enter the element tree"))
}

fn validate_live_element<'a>(
    document: &'a LynxDocument,
    function: &str,
    node: dom::NodeId,
) -> Result<&'a dom::Node<()>, String> {
    let node = document
        .get(node)
        .ok_or_else(|| format!("{function} received a stale element id"))?;
    if node.is_element() {
        Ok(node)
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
mod tests;
