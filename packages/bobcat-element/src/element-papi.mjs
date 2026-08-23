// @ts-check
import {
  attributeNames,
  childElementIds,
  createElement,
  createPage,
  disableEventListener,
  dropElement,
  enableEventListener,
  flushElementTree,
  getAttribute,
  insertBefore,
  parentNode,
  removeAttribute,
  removeElement,
  replaceElement,
  setAttribute,
  setInlineStyles,
  stopPropagation,
  swapElement,
  tagName,
} from "bobcat-internal:host";

// The Lynx Element PAPI runtime.
//
// Preloaded as the `bobcat:element` ESM inside the QuickJS main-thread realm;
// its Rstest suite imports the same bytes. It reaches native code only through
// named exports of the native `bobcat-internal:host` ESM. Named exports are
// the MTS bindings; this module installs no Element-PAPI globals.
//
// # Element PAPI scope
//
// | PAPI | Backed by |
// | --- | --- |
// | `__CreatePage(componentID, componentCSSID)` | native `createPage` export |
// | `__CreateElement(tag, parentComponentUniqueID)` | native `createElement` export |
// | `__CreateWrapperElement(parentComponentUniqueID)` | native `createElement` export |
// | `__CreateText(parentComponentUniqueID)` | native `createElement` export |
// | `__CreateImage(parentComponentUniqueID)` | native `createElement` export |
// | `__CreateView(parentComponentUniqueID)` | native `createElement` export |
// | `__CreateScrollView(parentComponentUniqueID)` | native `createElement` export |
// | `__CreateRawText(text)` | native `createElement` + `setAttribute` exports |
// | `__CreateList(parentComponentUniqueID, ...)` | native `createElement` export + this runtime's own store |
// | `__AppendElement(parent, child)` | native `insertBefore` export |
// | `__InsertElementBefore(parent, child, reference?)` | native `insertBefore` export |
// | `__RemoveElement(parent, child)` | native `removeElement` export |
// | `__ReplaceElement(newElement, oldElement)` | native `replaceElement` export |
// | `__ReplaceElements(parent, newChildren, oldChildren?)` | native `parentNode` + `insertBefore` + `removeElement` exports |
// | `__SwapElement(childA, childB)` | native `swapElement` / `replaceElement` exports |
// | `__SetClasses(element, classNames)` | native `setAttribute` / `removeAttribute` exports |
// | `__SetID(element, id)` | native `setAttribute` / `removeAttribute` exports |
// | `__GetID(element)` | native `getAttribute` export |
// | `__GetTag(element)` | native `tagName` export |
// | `__GetChildren(element)` | native `childElementIds` export + this runtime's handle index |
// | `__GetAttributeByName(element, name)` | native `getAttribute` export |
// | `__GetAttributeNames(element)` | native `attributeNames` export |
// | `__GetElementUniqueID(element)` | the handle's own node id |  // (= its Lynx unique id)
// | `__SetInlineStyles(element, value)` | native `setAttribute` / `removeAttribute` / `setInlineStyles` exports |
// | `__SetCSSId(elements, cssId, entryName?)` | nothing — accepted and ignored |
// | `__SetAttribute(element, name, value)` | native `setAttribute` / `removeAttribute` exports |
// | `__UpdateListCallbacks(list, ...)` | this runtime's own store |
// | `__AddEvent(element, type, name, handler)` | this runtime's own store |
// | `__GetEvent(element, name, type)` | this runtime's own store |
// | `__GetEvents(element)` | this runtime's own store |
// | `__SetEvents(element, events)` | this runtime's own store |
// | `__AddEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__RemoveEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__StopPropagation(event)` | native `stopPropagation` export |
// | `__StopImmediatePropagation(event)` | native `stopPropagation` export + this runtime's own store |
// | `__FlushElementTree()` | native `flushElementTree` export |
//
// Everything else — `__CreateFrame`, `__DropElement` (absent from every
// web-core generation), `__AddClass`, `__AddInlineStyle`, the dataset,
// component-info, config, template-part and animation members, the rest of
// tree querying (`__GetParent`, `__FirstElement`, `__LastElement`,
// `__NextElement`, `__ElementIsEqual`, `__GetPageElement`, `__GetAttributes`),
// selector querying, and list cell recycling — is not implemented. A bundle
// that reaches for another member fails at the missing global, not silently.
//
// `__SetCSSId` is the one member installed as a sink. It names the author-CSS
// scope its elements cascade in, and no layer lowers a decoded `StyleInfo`
// into scoped author rules yet — every fragment mounts globally, as web-core
// itself emits for an `enableRemoveCSSScope = true` bundle. Recording the id
// would mean choosing an encoding (web-core writes `l-css-id`/`l-e-name`
// attributes; native Lynx keeps css_id on the element) with no consumer to
// validate it against. A compiled card calls it while installing its snapshot
// runtime, so it accepts the call and drops the id rather than failing at a
// missing global; the scoping behavior lands with the ingestion side that
// reads it.
//
// # Events
//
// An element handle is an `EventTarget`. Listeners are JavaScript closures
// filed on the handle itself, under this file's symbols, and they die with
// it — nothing about a handler ever crosses into Rust. On the handle rather
// than in a `WeakMap` keyed by it, because QuickJS's `WeakMap` holds its
// values strongly whatever becomes of the key (it marks every value; it is
// not an ephemeron), so a listener that captured its own element would have
// kept the handle — and through it the element, and everything the element
// holds — alive for the life of the realm. A closure reachable only from the
// handle it captures is a cycle with no root, which the collector does free.
//
// Registration is the standard's: identity is (element, name, callback,
// capture), a second add of those four is ignored outright, and `once` and
// capture behave as `addEventListener`'s do. What is not the standard's is
// that the host has to be told a node is worth visiting. A list going from
// empty to occupied calls the native `enableEventListener` export and back to
// empty calls `disableEventListener`, so the host
// keeps an index and skips every node that has nothing registered — the walk
// crosses the boundary only where a listener actually is.
//
// Dispatch is the host's walk and this file's per-node work. The host
// computes the whole event path while it holds the document, releases it, and
// then calls this module's `__BobcatDispatchEvent(node, target, phase, name,
// detail, eventId, isLastCall)` export once per node per pass. Releasing first is
// what lets a callback mutate the tree. `phase` is the *pass* (`0` bubble, `1`
// capture), not the standard's `eventPhase`. They are different numbers, and
// at the target they do not even correspond, since both passes visit it; the
// event object's `eventPhase` is derived here, where the event object is.
//
// `eventId` names the dispatch and `isLastCall` says whether another call
// carries that id, which is what lets one event object live for the whole
// walk instead of one per node. A listener that writes a property onto the
// event is seen by the next listener, as a real `Event` gives. The host
// retains nothing of it: the object is held here, keyed by the id, and dropped
// on whichever of the walk's three endings comes first — the last call, a
// listener stopping propagation, or a listener throwing. The latter two are
// visible here as they happen, which is why the host only has to signal the
// first. Dropping runs the standard's last dispatch step first — `eventPhase`
// back to `NONE`, `currentTarget` to null — so an event a listener kept past
// the walk does not go on naming the node the walk stopped on.
//
// Propagation splits along the same line. Both stop methods call
// the native `stopPropagation` export, since the standard's `stopImmediatePropagation`
// implies `stopPropagation` and ending the walk is the host's. What stays
// here is the immediate half — skipping the rest of *this* node's listeners —
// because one delivery covers a whole node and the host has no finer step.
//
// # `__AddEvent`, the other registration form
//
// `__AddEventListener` files a callable under the standard's identity.
// `__AddEvent` files *one* handler per event name under a Lynx dispatch form
// — `bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`,
// `global-bindEvent` — replacing whatever that name held. It is the form
// ReactLynx's compiled output uses for every `bind*`/`catch*` prop, so it is
// the one a real card exercises.
//
// The two share the host's index and the same per-node delivery. The form
// supplies what the standard's identity does not carry: which pass to file
// in, and whether the walk ends after this node. Everything else about it
// lives in this file, which is why no host member had to change for it.
//
// What a handler *is* decides whether it can run. A worklet runs, through the
// card's own `runWorklet`. A string is a background-thread handler name, and
// there is no background realm here to publish it to: it is filed, reported by
// `__GetEvent`, and never called — while a `catch` form filed that way still
// ends the walk, because ending it is the form's doing, not the handler's.
// Anything else non-nullish is ignored, neither filed nor clearing what the
// name held, which is web-core's behavior for it. Native Lynx would take a
// callable and file it as a Lepus handler; web-core has nowhere to run one,
// and matching web-core is the compatibility target.
//
// `global-bindEvent` is filed and never indexed. The host walks the event
// path and nothing else, so indexing a global handler would deliver it only
// when its element happened to be on that path — a subset neither native Lynx
// nor web-core produces. The separate pass that gives the form meaning is the
// host's to add.
//
// # What is deliberately absent
//
// The pieces of `__AddEventListener` that duplicate `__AddEvent`:
// `closure_type` selecting a background handler string, and `bind_type`
// selecting Lynx's `catch` forms. A card that wants either has `__AddEvent`.
//
// List cell recycling. `__CreateList` and `__UpdateListCallbacks` file the
// callbacks; their consumer, `__SetAttribute(element, "update-list-info", …)`,
// throws, because reproducing it needs the child at an index and the native
// boundary answers only `parentNode`.
//
// There is no `preventDefault` and no `cancelable`: Lynx dispatches no
// cancelable event, and suppressing a built-in behavior goes through gesture
// arbitration, on the separate `InputEvent::default_prevented` seam.
//
// # Identity and lifecycle
//
// - An element handle is a plain object carrying its DOM `NodeId` under a
//   realm-local symbol — web-core's `uniqueIdSymbol` shape; every PAPI
//   return of an element yields the same object it was created with.
//   `parentComponentUniqueID` and `__CreatePage`'s arguments are accepted
//   for PAPI shape and unused.
// - **That number is Lynx's `unique_id`, and nothing here mints it.** The
//   DOM issues it when the element is created and this runtime only carries
//   it: there is no counter in this file, and `__GetElementUniqueID` reports
//   back exactly what the creating PAPI was handed. One id space, one
//   authority. Older web-core generations kept a JS-side counter beside the
//   native id; that split does not exist here.
// - A `unique_id` is never reissued. Freeing an element retires its id for
//   the life of the document, so an id a bundle stashed in a variable after
//   its handle died can only ever name something gone, never a later element
//   that happens to sit in the freed one's storage. Nothing in this file has
//   to guard against that case because it cannot arise.
// - **A handle is the one thing that holds its element.** Collection is the
//   only way it lets go, web-core's model: every non-page handle is
//   registered with a FinalizationRegistry whose cleanup calls the native
//   `dropElement`, which frees that element and nothing else — its element
//   children are unlinked and go on as detached roots, each held by its own
//   handle, while the text node a `raw-text` reflects goes with it, because
//   no handle could ever name one. Cleanup runs as a pending job at the
//   host's job checkpoints, and never at realm teardown, so the last
//   committed tree survives the bootstrap realm.
// - **A handle holds the handles of its children**, in the set under
//   [`ownedChildren`], maintained by the six tree mutations. That is what
//   makes the rule above safe: the page's handle is permanent, so every
//   *connected* element's handle is reachable from it through this chain and
//   cannot be collected while its element is on screen. A ReactLynx list
//   recycling a cell — handing its elements from one snapshot instance to
//   another, then deleting the old `__elements` array — drops the card's own
//   references and no more; the elements stay because their parents hold
//   them. What ends a subtree is detaching it: `__RemoveElement` takes its
//   root out of its parent's set, and once the card lets go too, the whole
//   subtree's handles become unreachable together and are collected as one.
//   The set is unordered and holds nothing but membership — the tree's order
//   is the host's, and asking this side to mirror it would be a second
//   source of truth for the one thing the native tree already answers.
// - The link the other way is a **number**, the owner's node id, not a
//   reference: a child that pointed back at its parent would make every
//   parent/child pair a cycle, which only a collection can resolve, where
//   plain reference counting frees an unreachable subtree at once. The id is
//   resolved through [`handlesByNodeId`], which holds handles weakly.
// - **A handle reading as gone does not mean its element is.** QuickJS
//   answers a `WeakRef` from the refcount, while the cleanup that calls
//   `dropElement` is enqueued only by a collection, so between script
//   letting go of a handle and the next collection the handle is
//   unreachable and its element is fully allocated and fully parented. So
//   the graph answers exactly one question — *which live handle owns this
//   one* — and never "is this element attached", which only the host knows.
//   The same line is what a user-agent component owes this file: it may
//   build and tear down its own shadow content freely, but detaching one of
//   its host's *light* children behind script's back would leave that child
//   filed under a parent it no longer has, and held alive by it.
//   What it does answer soundly is the contrapositive the graph is built on:
//   an owner with no live handle cannot be connected, because a connected
//   element's handle is held by its parent's up to the permanent page
//   handle. So a child of one is off screen whatever the tree says, its
//   owner's set is gone, and its owner's pending `dropElement` will unlink
//   it — which is why filing it under nothing is safe, and why no native
//   operation is ever chosen from this graph.
// - No misuse is validated here: a foreign handle resolves to undefined
//   and the call crashes at the native boundary.

const nodeIdSymbol = Symbol("nodeId");

/**
 * Which listener list a registration belongs to; also the `type_id` the
 * native index is keyed by, so a node with only bubble listeners is skipped
 * entirely during the capture pass.
 */
const BUBBLE = 0;
const CAPTURE = 1;

/**
 * The standard's `Event.eventPhase` values. The host sends the *pass*, not
 * these: the two are not the same number, and at the target they do not even
 * correspond, since both passes visit it. Deriving one from the other is
 * this file's job because the event object is this file's.
 */
const NONE = 0;
const CAPTURING_PHASE = 1;
const AT_TARGET = 2;
const BUBBLING_PHASE = 3;

/**
 * Where a handle files its listeners: an event name to a pair of lists
 * indexed by [`BUBBLE`, `CAPTURE`]. A list holds `{ callback, once }` in
 * registration order, which is firing order.
 *
 * On the handle, not in a map keyed by it — see the header's note on
 * QuickJS's `WeakMap` — so a registration can never be what keeps its
 * element alive: it dies with the handle, and nothing else reaches it.
 *
 * @typedef {{ callback: Function, once: boolean, removed: boolean }} Registration
 * @typedef {Map<string, [Registration[], Registration[]]>} ListenerLists
 */
const listenersSymbol = Symbol("listeners");

/**
 * The `type` strings `__AddEvent` has to recognize, lowercased the way
 * web-core lowercases both halves on the way in. The fifth form,
 * `bindEvent`, is the default every other test falls through to and so is
 * never compared against.
 */
const CATCH_EVENT = "catchevent";
const CAPTURE_BIND = "capture-bind";
const CAPTURE_CATCH = "capture-catch";
const GLOBAL_BIND = "global-bindevent";

/**
 * Which of an element's two handler maps a `type` selects. Both are keyed
 * by event *name* alone, with the type carried inside the entry, which is
 * native Lynx's `static_events` / `global_bind_events` split
 * (`AttributeHolder::SetStaticEvent`) rather than web-core's (name, type)
 * pair. The consequence is native's: filing `catchtap` over `bindtap`
 * replaces it, and `__GetEvent` answers for the requested type only.
 */
const STATIC = 0;
const GLOBAL = 1;

/**
 * Where a handle files its `__AddEvent` handlers: at most one per name in
 * each map. On the handle, for the reason the listener lists are.
 *
 * @typedef {{ type: string, name: string, handler: unknown }} FiledHandler
 * @typedef {[Map<string, FiledHandler>, Map<string, FiledHandler>]} HandlerMaps
 */
const handlersSymbol = Symbol("handlers");

/**
 * What this file has last told the host about a (handle, name, pass).
 *
 * The host indexes a plain set of `(node, capture)` per name, so
 * `disableEventListener` is unconditional — it cannot know that the other
 * registration kind still wants the node visited. Two kinds file into that
 * one index, `__AddEventListener` closures and one `__AddEvent` handler, so
 * the decision belongs here, taken from both, with only the transitions
 * crossing the boundary. Filed on the handle, like the rest.
 *
 * @typedef {Map<string, [boolean, boolean]>} IndexedPasses
 */
const indexedSymbol = Symbol("indexed");

/**
 * The list callbacks `__CreateList` and `__UpdateListCallbacks` file.
 *
 * Nothing reads them yet. Their consumer is
 * `__SetAttribute(element, "update-list-info", …)`, which needs one
 * primitive the native boundary does not have: the child at an index. They
 * are retained rather than dropped because a callback dropped at
 * `__CreateList` time cannot be recovered later — the card hands each over
 * exactly once.
 *
 * @typedef {{
 *   componentAtIndex: unknown,
 *   enqueueComponent: unknown,
 *   componentAtIndexes: unknown,
 * }} ListCallbacks
 */
const listCallbacksSymbol = Symbol("listCallbacks");

/**
 * The handles of a handle's children: an unordered strong set, which is what
 * keeps a connected element's handle from being collected under it.
 *
 * Membership only. Order is the native tree's, and the six tree mutations
 * that maintain this set never touch it — mirroring it here would be a second
 * answer to a question the host already answers, and the two could disagree.
 *
 * Created on first use, because most elements are leaves.
 *
 * @typedef {Set<object>} OwnedChildren
 */
const ownedChildrenSymbol = Symbol("ownedChildren");

/**
 * The node id of the handle whose [`ownedChildren`] holds this one.
 *
 * A number rather than the handle itself: a strong link back would make every
 * parent/child pair a reference cycle, and a cycle is freed only by a
 * collection, where an unreachable subtree of one-way links is freed by
 * reference counting the moment script lets go of its root. Resolved through
 * [`handlesByNodeId`], which is weak, so an owner whose handle has died reads
 * as no owner at all — see the header.
 */
const ownerSymbol = Symbol("owner");

/**
 * A handle as this file sees it: the node id, plus whatever it has filed
 * on the handle under its own symbols.
 *
 * @typedef {{ [key: symbol]: unknown }} Handle
 */

/**
 * @param {object} handle
 * @returns {Handle}
 */
function slotsOf(handle) {
  return /** @type {Handle} */ (handle);
}

/**
 * @param {object} handle
 * @returns {ListenerLists | undefined}
 */
function listenersOf(handle) {
  return /** @type {ListenerLists | undefined} */ (
    slotsOf(handle)[listenersSymbol]
  );
}

/**
 * @param {object} handle
 * @returns {HandlerMaps | undefined}
 */
function handlersOf(handle) {
  return /** @type {HandlerMaps | undefined} */ (
    slotsOf(handle)[handlersSymbol]
  );
}

/**
 * @param {object} handle
 * @returns {IndexedPasses | undefined}
 */
function indexedOf(handle) {
  return /** @type {IndexedPasses | undefined} */ (
    slotsOf(handle)[indexedSymbol]
  );
}

/**
 * The reverse of a handle's `nodeIdSymbol`: dispatch arrives from the host
 * naming a `NodeId`, and the handler store is keyed by handle.
 *
 * Held weakly, and cleared in the same sweep that releases the element, so
 * this index cannot become the reference that keeps a handle — and through
 * it an element — alive. The per-handle stores keep the same rule by living
 * on the handle, and for the same reason: collection stays the only way a
 * handle lets go.
 *
 * @type {Map<number, WeakRef<object>>}
 */
const handlesByNodeId = new Map();

/**
 * Frees the element of a handle that is gone.
 *
 * The host frees that element alone: its element children are unlinked into
 * detached roots for their own handles, and only what no handle could name —
 * the text node a `raw-text` reflects — goes with it. The element cannot
 * still be connected, because a connected element's handle is held by its
 * parent's, up to the permanent page handle.
 */
const registry = new FinalizationRegistry(
  (/** @type {number} */ nodeId) => {
    handlesByNodeId.delete(nodeId);
    dropElement(nodeId);
  },
);

/** @type {object | undefined} */
let pageHandle;

/**
 * @param {number} nodeId
 * @returns {object}
 */
function createHandle(nodeId) {
  const handle = { [nodeIdSymbol]: nodeId };
  handlesByNodeId.set(nodeId, new WeakRef(handle));
  registry.register(handle, nodeId, handle);
  return handle;
}

/**
 * The live handle for a node id, or undefined once its handle is gone.
 *
 * A swept-but-not-yet-finalized handle leaves a dead `WeakRef` behind, so
 * the entry is dropped on the way past rather than waiting for the cleanup
 * job that will release the element too.
 *
 * Undefined is an answer only where a node genuinely has nothing to say: a
 * path node with no handle is skipped, since its listeners lived on the
 * handle and went with it. Where a handle is *required* — the target and
 * `currentTarget` of a dispatch, or a query member that must return one
 * (`__GetParent` and its kind, none implemented) — undefined is not an
 * answer. No second handle is ever minted for a node whose first has died,
 * so those must throw: the alternative is the host holding a node no handle
 * names.
 *
 * @param {number} nodeId
 * @returns {object | undefined}
 */
function handleOf(nodeId) {
  const reference = handlesByNodeId.get(nodeId);
  if (reference === undefined) {
    return undefined;
  }
  const handle = reference.deref();
  if (handle === undefined) {
    handlesByNodeId.delete(nodeId);
  }
  return handle;
}

/**
 * @param {unknown} handle
 * @returns {number}
 */
function nodeIdOf(handle) {
  return /** @type {number} */ (
    /** @type {Record<symbol, unknown>} */ (handle)[nodeIdSymbol]
  );
}

/**
 * The live handle that owns `handle`, or undefined when none does.
 *
 * Undefined is **not** "detached" — see the header. It covers two cases: no
 * owner was ever recorded, and the recorded owner's handle is gone. Only the
 * second is subtle, and what it means is that the owner is unreachable from
 * script and cannot be connected, so nothing this file files under it would
 * ever be read again.
 *
 * @param {unknown} handle
 * @returns {object | undefined}
 */
function ownerOf(handle) {
  const owner = slotsOf(/** @type {object} */ (handle))[ownerSymbol];
  if (owner === undefined) {
    return undefined;
  }
  return handleOf(/** @type {number} */ (owner));
}

/**
 * Takes `handle` out of its owner's child set, if it has one.
 *
 * @param {unknown} handle
 * @returns {undefined}
 */
function disown(handle) {
  const slots = slotsOf(/** @type {object} */ (handle));
  const owner = slots[ownerSymbol];
  if (owner === undefined) {
    return undefined;
  }
  slots[ownerSymbol] = undefined;
  const parent = handleOf(/** @type {number} */ (owner));
  if (parent === undefined) {
    // Its handle is already gone, and its child set with it.
    return undefined;
  }
  /** @type {OwnedChildren | undefined} */ (
    slotsOf(parent)[ownedChildrenSymbol]
  )?.delete(/** @type {object} */ (handle));
  return undefined;
}

/**
 * Files `child` in `parent`'s child set, taking it out of whichever set held
 * it before. Called after the native mutation, so a call the host refuses
 * leaves this side exactly as the tree it failed to change.
 *
 * @param {unknown} parent
 * @param {unknown} child
 * @returns {undefined}
 */
function adopt(parent, child) {
  disown(child);
  const slots = slotsOf(/** @type {object} */ (parent));
  let owned = /** @type {OwnedChildren | undefined} */ (
    slots[ownedChildrenSymbol]
  );
  if (owned === undefined) {
    owned = new Set();
    slots[ownedChildrenSymbol] = owned;
  }
  owned.add(/** @type {object} */ (child));
  slotsOf(/** @type {object} */ (child))[ownerSymbol] = nodeIdOf(parent);
  return undefined;
}

/**
 * Files `child` under the handle for `parentNodeId`, the host's answer for
 * where the child now is.
 *
 * A parent with no live handle is one script has let go of, which cannot be
 * connected, and whose pending `dropElement` will unlink the child anyway:
 * there is no set to file it in and nothing is lost by not having one.
 *
 * @param {number} parentNodeId
 * @param {unknown} child
 * @returns {undefined}
 */
function adoptUnder(parentNodeId, child) {
  const parent = handleOf(parentNodeId);
  if (parent === undefined) {
    disown(child);
    return undefined;
  }
  adopt(parent, child);
  return undefined;
}

/**
 * @param {unknown} componentID
 * @param {unknown} componentCSSID
 * @returns {object}
 */
export function __CreatePage(componentID, componentCSSID) {
  void componentID;
  void componentCSSID;
  const nodeId = createPage();
  if (pageHandle === undefined) {
    // The page handle is permanent and exempt from the collection
    // backstop: the page can never be dropped. It is still indexed, because
    // an event whose path reaches the page has to find it.
    pageHandle = { [nodeIdSymbol]: nodeId };
    handlesByNodeId.set(nodeId, new WeakRef(pageHandle));
  }
  return pageHandle;
}

/**
 * @param {unknown} tag
 * @param {unknown} parentComponentUniqueID
 * @returns {object}
 */
export function __CreateElement(tag, parentComponentUniqueID) {
  void parentComponentUniqueID;
  return createHandle(createElement(/** @type {string} */ (tag)));
}

/**
 * @param {string} tag
 * @param {unknown} parentComponentUniqueID
 * @returns {object}
 */
function createTag(tag, parentComponentUniqueID) {
  void parentComponentUniqueID;
  return createHandle(createElement(tag));
}

/** @param {unknown} parentComponentUniqueID */
export function __CreateWrapperElement(parentComponentUniqueID) {
  return createTag("wrapper", parentComponentUniqueID);
}

/** @param {unknown} parentComponentUniqueID */
export function __CreateText(parentComponentUniqueID) {
  return createTag("text", parentComponentUniqueID);
}

/** @param {unknown} parentComponentUniqueID */
export function __CreateImage(parentComponentUniqueID) {
  return createTag("image", parentComponentUniqueID);
}

/** @param {unknown} parentComponentUniqueID */
export function __CreateView(parentComponentUniqueID) {
  return createTag("view", parentComponentUniqueID);
}

/** @param {unknown} parentComponentUniqueID */
export function __CreateScrollView(parentComponentUniqueID) {
  return createTag("scroll-view", parentComponentUniqueID);
}

/**
 * @param {unknown} text
 * @returns {object}
 */
export function __CreateRawText(text) {
  // The handle first, then the attribute. Nothing but a handle holds an
  // element, so a host call that throws in between would leave a node no
  // one could ever name or free — and `setAttribute` does throw, for a
  // value that is not a string.
  const handle = createHandle(createElement("raw-text"));
  setAttribute(nodeIdOf(handle), "text", /** @type {string} */ (text));
  return handle;
}

/**
 * List construction files the recycling callbacks the same way
 * `__UpdateListCallbacks` does; nothing reads them yet (see
 * [`listCallbacks`]).
 *
 * The rest parameter is what native declares as arguments 4 and 5: an
 * unused options object and the `componentAtIndexes` callback, which
 * ReactLynx passes here and not only through `__UpdateListCallbacks`. It
 * stays a rest parameter so the reported arity remains web-core's three.
 *
 * @param {unknown} parentComponentUniqueID
 * @param {unknown} componentAtIndex
 * @param {unknown} enqueueComponent
 * @param {unknown[]} rest `[info, componentAtIndexes]`
 * @returns {object}
 */
export function __CreateList(
  parentComponentUniqueID,
  componentAtIndex,
  enqueueComponent,
  ...rest
) {
  void parentComponentUniqueID;
  const handle = createHandle(createElement("list"));
  slotsOf(handle)[listCallbacksSymbol] = /** @type {ListCallbacks} */ ({
    componentAtIndex,
    enqueueComponent,
    componentAtIndexes: rest[1],
  });
  return handle;
}

/**
 * @param {unknown} parent
 * @param {unknown} child
 * @returns {object}
 */
export function __AppendElement(parent, child) {
  insertBefore(nodeIdOf(parent), nodeIdOf(child), null);
  adopt(parent, child);
  return /** @type {object} */ (child);
}

/**
 * @param {unknown} parent
 * @param {unknown} child
 * @param {unknown} reference
 * @returns {object}
 */
export function __InsertElementBefore(parent, child, reference) {
  if (reference === child) {
    return /** @type {object} */ (child);
  }
  insertBefore(
    nodeIdOf(parent),
    nodeIdOf(child),
    reference === undefined || reference === null
      ? null
      : nodeIdOf(reference),
  );
  adopt(parent, child);
  return /** @type {object} */ (child);
}

/**
 * @param {unknown} parent
 * @param {unknown} child
 * @returns {object}
 */
export function __RemoveElement(parent, child) {
  void parent;
  removeElement(nodeIdOf(child));
  disown(child);
  return /** @type {object} */ (child);
}

/**
 * web-core's algorithm: without old children this is a plain append; with
 * them, every old child after the first is detached and the first is
 * replaced in place — under its actual parent, a no-op when detached,
 * exactly `ChildNode.replaceWith`.
 *
 * "Its actual parent" is the host's answer, not the ownership graph's. The
 * graph cannot answer it — a handle script let go of reads as gone while its
 * element is still there and still a parent (see the header) — and choosing
 * a *different native operation* on that reading is how an element with a
 * live handle under a let-go parent would get treated as detached.
 *
 * @param {unknown} parent
 * @param {unknown} newChildren
 * @param {unknown} oldChildren
 * @returns {undefined}
 */
export function __ReplaceElements(parent, newChildren, oldChildren) {
  const news = Array.isArray(newChildren) ? newChildren : [newChildren];
  if (!oldChildren || (Array.isArray(oldChildren) && oldChildren.length === 0)) {
    const parentNodeId = nodeIdOf(parent);
    for (const child of news) {
      insertBefore(parentNodeId, nodeIdOf(child), null);
      adopt(parent, child);
    }
    return undefined;
  }
  const olds = Array.isArray(oldChildren) ? oldChildren : [oldChildren];
  for (let index = 1; index < olds.length; index += 1) {
    removeElement(nodeIdOf(olds[index]));
    disown(olds[index]);
  }
  const first = nodeIdOf(olds[0]);
  const actualParent = parentNode(first);
  if (actualParent === null) {
    return undefined;
  }
  for (const child of news) {
    insertBefore(actualParent, nodeIdOf(child), first);
    adoptUnder(actualParent, child);
  }
  removeElement(first);
  disown(olds[0]);
  return undefined;
}

/**
 * The native swap covers the simple case, two distinct attached elements;
 * the degenerate patterns of web-core's transient-marker algorithm are
 * composed here: a self-swap does nothing, a detached operand takes the
 * attached one's place and detaches it, two detached operands are left
 * alone.
 *
 * @param {unknown} childA
 * @param {unknown} childB
 * @returns {undefined}
 */
export function __SwapElement(childA, childB) {
  const a = nodeIdOf(childA);
  const b = nodeIdOf(childB);
  if (a === b) {
    return undefined;
  }
  // Which pattern applies is the host's answer, for the reason
  // `__ReplaceElements` gives. Both parents are read before the swap,
  // because the swap is what exchanges them, and answer the bookkeeping too.
  const parentA = parentNode(a);
  const parentB = parentNode(b);
  if (parentA !== null && parentB !== null) {
    swapElement(a, b);
    adoptUnder(parentB, childA);
    adoptUnder(parentA, childB);
  } else if (parentA !== null) {
    replaceElement(b, a);
    adoptUnder(parentA, childB);
    disown(childA);
  } else if (parentB !== null) {
    replaceElement(a, b);
    adoptUnder(parentB, childA);
    disown(childB);
  }
  return undefined;
}

/**
 * @param {unknown} newElement
 * @param {unknown} oldElement
 * @returns {undefined}
 */
export function __ReplaceElement(newElement, oldElement) {
  if (newElement === oldElement) {
    return undefined;
  }
  // The graph is read for bookkeeping only: it never decides what the host
  // is asked to do, so a stale-looking answer costs a filing, never a
  // different tree. A live owner is always the real parent — the only way an
  // element leaves one without a mutation saying so is that parent being
  // freed, which takes its handle with it.
  const owner = ownerOf(oldElement);
  replaceElement(nodeIdOf(newElement), nodeIdOf(oldElement));
  if (owner !== undefined) {
    adopt(owner, newElement);
  }
  disown(oldElement);
  return undefined;
}

/**
 * For an ordinary React-shaped property name, an uppercase letter becomes
 * `-` plus its lowercase form, so `backgroundColor` reaches CSS as
 * `background-color`. Custom-property names are case-sensitive CSS idents,
 * so an authored `--accentColor` must pass through unchanged.
 *
 * @param {string} name
 * @returns {string}
 */
function hyphenate(name) {
  if (name.startsWith("--")) {
    return name;
  }
  return name.replace(
    /[A-Z]/g,
    (character) => `-${character.toLowerCase()}`,
  );
}

/**
 * web-core's truthiness test, not a null check: an empty class list removes
 * the attribute, which is how ReactLynx clears one.
 *
 * @param {unknown} element
 * @param {unknown} classNames
 * @returns {undefined}
 */
export function __SetClasses(element, classNames) {
  const nodeId = nodeIdOf(element);
  if (classNames) {
    setAttribute(nodeId, "class", String(classNames));
  } else {
    removeAttribute(nodeId, "class");
  }
  return undefined;
}

/**
 * @param {unknown} element
 * @param {unknown} id
 * @returns {undefined}
 */
export function __SetID(element, id) {
  const nodeId = nodeIdOf(element);
  if (id) {
    setAttribute(nodeId, "id", String(id));
  } else {
    removeAttribute(nodeId, "id");
  }
  return undefined;
}

/**
 * @param {unknown} element
 * @returns {string | null}
 */
export function __GetID(element) {
  return getAttribute(nodeIdOf(element), "id");
}

/**
 * The element's Lynx tag. web-core maps its HTML stand-in back
 * (`x-view` -> `view`); this runtime creates elements under the Lynx tag
 * itself, so the DOM's own local name is already the answer.
 *
 * @param {unknown} element
 * @returns {string}
 */
export function __GetTag(element) {
  return tagName(nodeIdOf(element));
}

/**
 * One attribute's value, or null when the element does not carry it.
 *
 * `__GetID` is this member with the name fixed, and both read the same
 * native export: an id is an attribute here, not a field beside them.
 *
 * @param {unknown} element
 * @param {unknown} name
 * @returns {string | null}
 */
export function __GetAttributeByName(element, name) {
  return getAttribute(nodeIdOf(element), String(name));
}

/**
 * Every attribute name the element carries, in the order it acquired them —
 * `getAttributeNames()`' order, and the order `attributes()` reports.
 *
 * `class`, `id` and `style` are in it. They reach the DOM through paths of
 * their own — a class list, an id atom, a parsed declaration block — but each
 * of those paths also writes the attribute itself, so the list this reads is
 * the whole list rather than the leftovers.
 *
 * @param {unknown} element
 * @returns {string[]}
 */
export function __GetAttributeNames(element) {
  return splitRecord(attributeNames(nodeIdOf(element)));
}

/**
 * The element's element children, in tree order.
 *
 * Element children, not child nodes: a `raw-text`'s content is a DOM text
 * node, and no handle names it, so reporting it could only ever produce a
 * hole. Filtering to elements is therefore what keeps the "every child has a
 * handle" invariant below true for an ordinary tree rather than a lucky one.
 *
 * A child of a live parent has a live handle: a connected element's handle is
 * held by its parent's, up to the permanent page handle, and the caller had
 * to hold the parent's handle to make this call. So the throw is a statement
 * about the invariant, not a case a card can reach — and it is a throw rather
 * than a hole because minting a second handle for a node whose first has died
 * would leave the host holding a node no handle names.
 *
 * @param {unknown} element
 * @returns {object[]}
 */
export function __GetChildren(element) {
  const record = childElementIds(nodeIdOf(element));
  if (record === "") {
    return [];
  }
  return record.split(",").map((field) => {
    const nodeId = Number(field);
    const handle = handleOf(nodeId);
    if (handle === undefined) {
      throw new Error(
        `__GetChildren found no live handle for child element ${nodeId}`,
      );
    }
    return handle;
  });
}

/**
 * The element's Lynx `unique_id`, which is the same number as its native
 * node id — the handle carries one value and this reads it back, rather
 * than mapping between two id spaces.
 *
 * The one query web-core answers instead of crashing: a falsy or foreign
 * element reports `-1` rather than throwing, which is the contract its
 * callers read. `-1` is safe as the sentinel precisely because real ids
 * are issued from zero upward and never recycled.
 *
 * @param {unknown} element
 * @returns {number}
 */
export function __GetElementUniqueID(element) {
  if (!element) {
    return -1;
  }
  return nodeIdOf(element) ?? -1;
}

/**
 * Reads a record the native side wrote back — the same
 * `<utf16Length>:<text>` fields [`styleField`] writes, in the other
 * direction, so a field may contain any character including the delimiter.
 * `String.prototype.slice` counts the units the writer counted, so each
 * field costs one slice and no scan.
 *
 * Nothing here validates the payload. The writer is Bobcat, not a card: a
 * malformed record would be an engine bug, and reporting it as a JavaScript
 * error would only move it further from where it happened.
 *
 * @param {string} record
 * @returns {string[]}
 */
function splitRecord(record) {
  /** @type {string[]} */
  const fields = [];
  let rest = record;
  while (rest !== "") {
    const separator = rest.indexOf(":");
    const units = Number(rest.slice(0, separator));
    const body = rest.slice(separator + 1);
    fields.push(body.slice(0, units));
    rest = body.slice(units);
  }
  return fields;
}

/**
 * Encodes one field of a style record: its length in UTF-16 code units,
 * a colon, then the text.
 *
 * `String.prototype.length` already counts the units the native side
 * decodes, so this costs one property read and no scan. The length is what
 * lets a value contain any character at all — a semicolon, a quote, a NUL
 * — without a delimiter having to be escaped or a declaration boundary
 * having to be guessed.
 *
 * @param {string} text
 * @returns {string}
 */
function styleField(text) {
  return `${text.length}:${text}`;
}

/**
 * A string is one complete style-attribute payload and is set verbatim. A
 * record is still a complete replacement, and crosses in one call: the
 * hyphenated names and stringified values are packed into a single
 * self-describing payload, and the native side builds the declaration block
 * from empty.
 *
 * The fan-out used to live here, one CSSOM-like `setProperty` per value.
 * That cost one crossing per property *and* made the native side clone and
 * re-serialize the whole block for each one, so an `n`-property record was
 * quadratic. Replacement semantics make the reset implicit: there is no
 * old block to preserve, so there is nothing for the fan-out to mutate.
 *
 * A falsy value removes the attribute. The `rpx`/`vw`/`vh`/`rem` token
 * rewriting web-core performs on the way through has no owner here yet, so
 * declarations reach stylo as authored.
 *
 * @param {unknown} element
 * @param {unknown} value
 * @returns {undefined}
 */
export function __SetInlineStyles(element, value) {
  const nodeId = nodeIdOf(element);
  if (!value) {
    removeAttribute(nodeId, "style");
    return undefined;
  }
  if (typeof value === "string") {
    setAttribute(nodeId, "style", value);
    return undefined;
  }
  // Object enumeration order is the declaration order, so shorthand and
  // longhand precedence within the record is kept. An empty or all-nullish
  // record sends an empty payload, which leaves an empty `style` attribute
  // — observable, and what web-core's complete-record setter does.
  const fields = [];
  for (const [key, declaration] of Object.entries(value)) {
    if (declaration === null || declaration === undefined) {
      continue;
    }
    fields.push(styleField(hyphenate(key)), styleField(String(declaration)));
  }
  setInlineStyles(nodeId, fields.join(""));
  return undefined;
}

/**
 * Accepted and ignored.
 *
 * The PAPI names the author-CSS scope a set of elements cascades in, and
 * nothing lowers a decoded `StyleInfo` into scoped author rules yet: every
 * fragment mounts globally, which is what web-core itself emits for an
 * `enableRemoveCSSScope = true` bundle. Recording the id would therefore
 * mean choosing an encoding — web-core writes `l-css-id`/`l-e-name`
 * attributes, native Lynx keeps `css_id` on the element — with no consumer
 * to validate the choice against, so the id is dropped rather than written
 * somewhere a later scoping pass would have to unlearn.
 *
 * It is a sink rather than an absence because a compiled card calls it
 * while installing its snapshot runtime, and a card whose styles are all
 * global has nothing to gain from failing at the missing global. The scoped
 * behavior lands with the ingestion side that reads it.
 *
 * @param {unknown} elements
 * @param {unknown} cssId
 * @param {unknown} entryName
 * @returns {undefined}
 */
export function __SetCSSId(elements, cssId, entryName) {
  void elements;
  void cssId;
  void entryName;
  return undefined;
}

/**
 * `null`/`undefined` removes; anything else is stringified, which is what
 * web-core's `setElementPropertyOrAttribute` does for every name that is not
 * a live property of its HTML stand-in element. `id`, `class`, and `style`
 * reach their specialized DOM paths inside the native `setAttribute` export.
 *
 * `update-list-info` is the one name that is not an attribute at all: it
 * drives list cell insertion and removal, and throws here rather than
 * writing a stringified command object onto the element. The callbacks it
 * would drive are filed (see [`listCallbacks`]); what is missing is the
 * child at an index, which the native boundary cannot answer.
 *
 * @param {unknown} element
 * @param {unknown} name
 * @param {unknown} value
 * @returns {undefined}
 */
export function __SetAttribute(element, name, value) {
  if (name === "update-list-info") {
    throw new Error(
      "__SetAttribute(update-list-info) needs indexed child access, which the native boundary does not have",
    );
  }
  const nodeId = nodeIdOf(element);
  if (value === null || value === undefined) {
    removeAttribute(nodeId, String(name));
  } else {
    setAttribute(nodeId, String(name), String(value));
  }
  return undefined;
}

/**
 * The pass a `__AddEvent` type is delivered in. Lynx's four path-walking
 * forms collapse onto the two passes the host walks: the `capture-` pair is
 * the capture pass, `bindEvent`/`catchEvent` the bubble pass.
 * `global-bindEvent` is not one of them and never reaches this.
 *
 * @param {string} type
 * @returns {0 | 1}
 */
function phaseOfType(type) {
  return type === CAPTURE_BIND || type === CAPTURE_CATCH ? CAPTURE : BUBBLE;
}

/**
 * Whether a type ends the walk after its node. Both `catch` forms do, and
 * they do it because of what they are: native Lynx decides from the
 * registration's existence, not from what its handler did or whether one
 * ran at all.
 *
 * @param {string} type
 * @returns {boolean}
 */
function isCatchType(type) {
  return type === CATCH_EVENT || type === CAPTURE_CATCH;
}

/**
 * One element's two handler maps, created on demand.
 *
 * @param {object} handle
 * @returns {HandlerMaps}
 */
function handlersFor(handle) {
  let maps = handlersOf(handle);
  if (maps === undefined) {
    maps = [new Map(), new Map()];
    slotsOf(handle)[handlersSymbol] = maps;
  }
  return maps;
}

/**
 * Tells the host about one (element, name, pass), but only when the answer
 * changed. Records what was said, so the next reconciliation knows.
 *
 * @param {object} handle
 * @param {string} name
 * @param {0 | 1} phase
 * @param {boolean} wanted
 * @returns {undefined}
 */
function syncPass(handle, name, phase, wanted) {
  let byName = indexedOf(handle);
  const state = byName?.get(name);
  if (wanted === (state?.[phase] ?? false)) {
    return undefined;
  }
  if (wanted) {
    enableEventListener(nodeIdOf(handle), phase, name);
  } else {
    disableEventListener(nodeIdOf(handle), phase, name);
  }
  if (state !== undefined) {
    state[phase] = wanted;
    if (!state[BUBBLE] && !state[CAPTURE]) {
      byName?.delete(name);
    }
    return undefined;
  }
  if (byName === undefined) {
    byName = new Map();
    slotsOf(handle)[indexedSymbol] = byName;
  }
  const created = /** @type {[boolean, boolean]} */ ([false, false]);
  created[phase] = wanted;
  byName.set(name, created);
  return undefined;
}

/**
 * Reconciles both passes of the host's index for one (element, name)
 * against everything registered here now.
 *
 * @param {object} handle
 * @param {string} name
 * @returns {undefined}
 */
function syncIndex(handle, name) {
  const lists = listenersOf(handle)?.get(name);
  const filed = handlersOf(handle)?.[STATIC].get(name);
  const filedPhase = filed === undefined ? undefined : phaseOfType(filed.type);
  syncPass(
    handle,
    name,
    BUBBLE,
    (lists !== undefined && lists[BUBBLE].length > 0) ||
      filedPhase === BUBBLE,
  );
  syncPass(
    handle,
    name,
    CAPTURE,
    (lists !== undefined && lists[CAPTURE].length > 0) ||
      filedPhase === CAPTURE,
  );
  return undefined;
}

/**
 * The listener lists for one element and event name, created on demand.
 *
 * @param {object} handle
 * @param {string} name
 * @returns {[Registration[], Registration[]]}
 */
function listsFor(handle, name) {
  let byName = listenersOf(handle);
  if (byName === undefined) {
    byName = new Map();
    slotsOf(handle)[listenersSymbol] = byName;
  }
  let lists = byName.get(name);
  if (lists === undefined) {
    lists = [[], []];
    byName.set(name, lists);
  }
  return lists;
}

/**
 * `addEventListener`, with the standard's registration identity:
 * (element, name, callback, capture). A second add of the same four is
 * ignored outright — including its options, so re-adding with `once` neither
 * files a second listener nor changes the first.
 *
 * A list going from empty to occupied is what tells the host this node is
 * worth visiting for this name in this pass; until then the host skips it
 * and no cross-boundary call happens at all. The telling goes through
 * `syncIndex`, because an `__AddEvent` handler files into the same host
 * index and neither kind may switch the other off.
 *
 * @param {object} handle
 * @param {unknown} eventName
 * @param {unknown} callback
 * @param {unknown} options
 * @returns {undefined}
 */
function addListener(handle, eventName, callback, options) {
  if (typeof callback !== "function") {
    // web-core ignores a non-callable under the default closure type; a
    // string handler is a background-thread name, which is not delivered
    // here at all.
    return undefined;
  }
  const name = String(eventName).toLowerCase();
  const settings = /** @type {Record<string, unknown> | undefined} */ (
    options ?? undefined
  );
  const phase = settings?.["capture"] ? CAPTURE : BUBBLE;
  const list = listsFor(handle, name)[phase];
  if (list.some((registration) => registration.callback === callback)) {
    return undefined;
  }
  list.push({
    callback,
    once: Boolean(settings?.["once"]),
    removed: false,
  });
  syncIndex(handle, name);
  return undefined;
}

/**
 * `removeEventListener`. Capture is part of the identity, so a bubble-phase
 * removal leaves a capture registration of the same callback alone.
 *
 * @param {object} handle
 * @param {unknown} eventName
 * @param {unknown} callback
 * @param {unknown} options
 * @returns {undefined}
 */
function removeListener(handle, eventName, callback, options) {
  const name = String(eventName).toLowerCase();
  const byName = listenersOf(handle);
  const lists = byName?.get(name);
  if (lists === undefined) {
    return undefined;
  }
  const settings = /** @type {Record<string, unknown> | undefined} */ (
    options ?? undefined
  );
  const phase = settings?.["capture"] ? CAPTURE : BUBBLE;
  const list = lists[phase];
  const index = list.findIndex(
    (registration) => registration.callback === callback,
  );
  if (index === -1) {
    return undefined;
  }
  // Marked as well as spliced, because a dispatch in progress iterates a
  // copy: the standard says a listener removed by an earlier one must not
  // run, and the copy alone cannot know that.
  const [removed] = list.splice(index, 1);
  if (removed !== undefined) {
    removed.removed = true;
  }
  syncIndex(handle, name);
  return undefined;
}

/**
 * `__AddEvent`'s registration, shared with `__SetEvents`.
 *
 * The identity is (element, map, name) — the map being the one `type`
 * selects — so a second call for the same name replaces the first outright,
 * type included. That is `insert_or_assign` on native Lynx's event map, and
 * it is what ReactLynx's per-slot updater relies on: it rewrites the same
 * binding on every render rather than removing and re-adding it.
 *
 * A nullish handler is the removal, matching `FiberAddEvent`'s
 * empty-callback branch.
 *
 * @param {object} handle
 * @param {unknown} eventType
 * @param {unknown} eventName
 * @param {unknown} handler
 * @returns {undefined}
 */
function addEvent(handle, eventType, eventName, handler) {
  const type = String(eventType).toLowerCase();
  const name = String(eventName).toLowerCase();
  const slot = type === GLOBAL_BIND ? GLOBAL : STATIC;
  if (handler === null || handler === undefined) {
    handlersOf(handle)?.[slot].delete(name);
  } else if (typeof handler === "string" || typeof handler === "object") {
    handlersFor(handle)[slot].set(name, { type, name, handler });
  } else {
    // Neither a handler name nor a worklet. web-core's `__AddEvent` matches
    // none of its three branches on such a call and so does nothing at all
    // — it does not file, and it does not clear what the name already held.
    // A callable reaching here is native Lynx's Lepus handler; web-core has
    // no main-thread place to run one and this runtime does not invent one.
    return undefined;
  }
  if (slot === STATIC) {
    syncIndex(handle, name);
  }
  return undefined;
}

/**
 * Files one handler for one element, event name and Lynx dispatch form.
 *
 * Two handler kinds are filed, and they are not equally deliverable:
 *
 * - a **worklet** (`{ type: "worklet", value }`, what `main-thread:bind*`
 *   compiles to) runs here, through the `runWorklet` the card's own worklet
 *   runtime installs on this realm;
 * - a **string** is a background-thread handler *name*. There is no
 *   background realm to publish it to, so it is filed and never called. It
 *   is filed rather than rejected because a `catch` form still ends the
 *   walk, and because `__GetEvent` has to report what the card handed over.
 *
 * Anything else that is not nullish — a callable above all — is ignored
 * outright, which is what web-core does with it.
 *
 * `global-bindEvent` is filed in its own map and never indexed: the host
 * walks the event path and nothing else, so a global handler would only
 * ever be reached when its element happened to be on that path. Delivering
 * that subset would be a behavior neither native Lynx nor web-core has.
 * The pass that gives it meaning is the host's to add.
 *
 * @param {unknown} element
 * @param {unknown} eventType
 * @param {unknown} eventName
 * @param {unknown} handler
 * @returns {undefined}
 */
export function __AddEvent(element, eventType, eventName, handler) {
  return addEvent(
    /** @type {object} */ (element),
    eventType,
    eventName,
    handler,
  );
}

/**
 * The handler filed for one name, or undefined when the filed one belongs
 * to a different dispatch form — the type check `FiberGetEvent` performs,
 * which is only meaningful because the map is keyed by name alone.
 *
 * The arguments are (name, type), the reverse of `__AddEvent`'s
 * (type, name). Both native Lynx and web-core order them this way.
 *
 * @param {unknown} element
 * @param {unknown} eventName
 * @param {unknown} eventType
 * @returns {unknown}
 */
export function __GetEvent(element, eventName, eventType) {
  const type = String(eventType).toLowerCase();
  const name = String(eventName).toLowerCase();
  const slot = type === GLOBAL_BIND ? GLOBAL : STATIC;
  const filed = handlersOf(/** @type {object} */ (element))
    ?.[slot].get(name);
  if (filed === undefined || filed.type !== type) {
    return undefined;
  }
  return filed.handler;
}

/**
 * Every handler filed on one element, static map first.
 *
 * The three references disagree on the shape: native Lynx returns a record
 * of name to array, web-core's WASM returns records spelled
 * `event_name`/`event_type`/`event_handler`, and the PAPI type both declare
 * says `{ type, name, function }[]`. The declared shape wins here, because
 * it is the only one that makes `__SetEvents(e, __GetEvents(e))` a faithful
 * round trip — the one property any caller could rely on.
 *
 * @param {unknown} element
 * @returns {{ type: string, name: string, function: unknown }[]}
 */
export function __GetEvents(element) {
  const maps = handlersOf(/** @type {object} */ (element));
  if (maps === undefined) {
    return [];
  }
  /** @type {{ type: string, name: string, function: unknown }[]} */
  const events = [];
  for (const map of maps) {
    for (const filed of map.values()) {
      events.push({
        type: filed.type,
        name: filed.name,
        function: filed.handler,
      });
    }
  }
  return events;
}

/**
 * Replaces every handler on one element.
 *
 * `FiberSetEvents` clears the element's maps before it adds, and this does
 * too. web-core's version only loops `__AddEvent`, so a name absent from
 * the new list keeps its old handler; that is a divergence in web-core, not
 * a semantic worth carrying, because it makes the PAPI unable to express
 * the removal its name promises.
 *
 * An entry whose `name` or `type` is not a string is skipped, as native
 * does, and a non-array clears and stops.
 *
 * @param {unknown} element
 * @param {unknown} events
 * @returns {undefined}
 */
export function __SetEvents(element, events) {
  const handle = /** @type {object} */ (element);
  const maps = handlersOf(handle);
  if (maps !== undefined) {
    const names = [...maps[STATIC].keys(), ...maps[GLOBAL].keys()];
    maps[STATIC].clear();
    maps[GLOBAL].clear();
    for (const name of names) {
      syncIndex(handle, name);
    }
  }
  if (!Array.isArray(events)) {
    return undefined;
  }
  for (const event of events) {
    const record = /** @type {Record<string, unknown>} */ (event);
    if (
      typeof record?.["name"] !== "string" ||
      typeof record["type"] !== "string"
    ) {
      continue;
    }
    addEvent(handle, record["type"], record["name"], record["function"]);
  }
  return undefined;
}

/**
 * Files a list element's recycling callbacks, replacing whatever
 * `__CreateList` or an earlier call left. ReactLynx passes null for all
 * three when it tears a list down.
 *
 * Storage only: see [`listCallbacks`] for why nothing reads them yet.
 *
 * @param {unknown} list
 * @param {unknown} componentAtIndex
 * @param {unknown} enqueueComponent
 * @param {unknown} componentAtIndexes
 * @returns {undefined}
 */
export function __UpdateListCallbacks(
  list,
  componentAtIndex,
  enqueueComponent,
  componentAtIndexes,
) {
  slotsOf(/** @type {object} */ (list))[listCallbacksSymbol] =
    /** @type {ListCallbacks} */ ({
      componentAtIndex,
      enqueueComponent,
      componentAtIndexes,
    });
  return undefined;
}

/**
 * The identity half of an event's `target`/`currentTarget`.
 *
 * `elementRefptr` is the handle itself, which a main-thread callback is
 * entitled to — it is in the same realm and already holds one. `dataset` is
 * absent: it is every `data-*` attribute, and the native boundary reads one
 * named attribute at a time with no way to enumerate.
 *
 * A node the host routed an event to is connected, and a connected element's
 * handle is held by its parent's up to the permanent page handle, so one
 * exists. If it does not, the ownership graph and the tree disagree: the
 * event cannot name what it happened to, and nothing this file could return
 * would be better than saying so.
 *
 * The one node kind that is connected and yet unnameable is a UA component's
 * shadow content, which script never sees. No component has a shadow root
 * today; the first one with hit-testable chrome owes the host a retarget to
 * its host element before the path is built, which is also what a browser
 * reports as the target.
 *
 * @param {number} nodeId
 * @returns {object}
 */
function targetInfo(nodeId) {
  const handle = handleOf(nodeId);
  if (handle === undefined) {
    throw new Error(
      `no handle names element ${nodeId}: the element ownership graph and the tree disagree`,
    );
  }
  return {
    id: getAttribute(nodeId, "id"),
    uid: nodeId,
    elementRefptr: handle,
  };
}

/**
 * The event objects of the dispatches currently walking, by host event id.
 *
 * One entry, minted at a dispatch's first delivery and dropped at its last,
 * so every listener on a path sees the same object — a property one writes
 * is there for the next, which is what an `Event` instance is for. The host
 * cannot nest dispatches, so this holds at most one entry at a time; it is a
 * `Map` rather than a single slot only because the id is what identifies an
 * entry, and dropping the wrong one would be silent.
 *
 * @typedef {{
 *   type: string,
 *   eventPhase: number,
 *   target: object,
 *   currentTarget: object | null,
 *   detail: unknown,
 *   stopPropagation: () => void,
 *   stopImmediatePropagation: () => void,
 * }} DispatchedEvent
 *
 * @typedef {{
 *   event: DispatchedEvent,
 *   targetNodeId: number,
 *   immediate: boolean,
 *   stopped: boolean,
 * }} Dispatch
 *
 * @type {Map<number, Dispatch>}
 */
const dispatches = new Map();

/**
 * The entry for one dispatch, created on its first delivery.
 *
 * `detail` is parsed once here rather than once per node, and the two stop
 * methods close over the entry, so they keep working from a later delivery
 * of the same event.
 *
 * @param {number} eventId
 * @param {string} name
 * @param {number} targetNodeId
 * @param {unknown} detailJson
 * @returns {Dispatch}
 */
function dispatchEntry(eventId, name, targetNodeId, detailJson) {
  const existing = dispatches.get(eventId);
  if (existing !== undefined) {
    return existing;
  }
  const entry = /** @type {Dispatch} */ ({
    targetNodeId,
    immediate: false,
    stopped: false,
  });
  entry.event = {
    type: name,
    eventPhase: NONE,
    target: targetInfo(targetNodeId),
    currentTarget: null,
    detail: detailJson ? JSON.parse(String(detailJson)) : {},
    stopPropagation: () => {
      entry.stopped = true;
      stopPropagation();
    },
    stopImmediatePropagation: () => {
      entry.immediate = true;
      entry.stopped = true;
      stopPropagation();
    },
  };
  dispatches.set(eventId, entry);
  return entry;
}

/**
 * The standard's `eventPhase` for one step.
 *
 * A step whose target is itself is at-target — crossing a shadow boundary
 * sets the target to the node it crossed to, and nothing else makes the two
 * equal — and both passes visit it. Every other step takes its phase from
 * the pass it belongs to.
 *
 * @param {number} node
 * @param {unknown} target
 * @param {number} phase
 * @returns {number}
 */
function eventPhaseOf(node, target, phase) {
  if (node === target) {
    return AT_TARGET;
  }
  return phase === CAPTURE ? CAPTURING_PHASE : BUBBLING_PHASE;
}

/**
 * One node's turn at one event, called by the host once per node per pass.
 *
 * The host owns the walk. It computes the whole event path while it holds
 * the document, releases it, and only then calls in here — so a callback is
 * free to mutate the tree, which is the reason for that order. What this
 * function owns is one node's listeners: which list the pass selects, their
 * order, `once`, and stopping the rest of them.
 *
 * Both stop methods reach `stopPropagation`, because the standard's
 * `stopImmediatePropagation` implies `stopPropagation` and ending the walk
 * is the host's to do. What does *not* leave is the immediate half — the
 * skipping of this node's remaining listeners — because one delivery covers
 * the whole node and the host has no finer step to withhold.
 *
 * @param {unknown} nodeId
 * @param {unknown} targetNodeId
 * @param {unknown} phaseId `CAPTURE` or `BUBBLE`, the pass being run
 * @param {unknown} eventName
 * @param {unknown} detailJson the event's device facts, or an empty string
 * @param {unknown} eventId names this dispatch, the same for all its calls
 * @param {unknown} isLastCall whether the host will call again for this id
 * @returns {undefined}
 */
function eventListenerCallback(
  nodeId,
  targetNodeId,
  phaseId,
  eventName,
  detailJson,
  eventId,
  isLastCall,
) {
  const id = Number(eventId);
  let entry;
  try {
    entry = deliverEvent(
      id,
      /** @type {number} */ (nodeId),
      /** @type {number} */ (targetNodeId),
      phaseId === CAPTURE ? CAPTURE : BUBBLE,
      String(eventName).toLowerCase(),
      detailJson,
    );
  } catch (error) {
    // A listener threw. The host aborts the walk on it, so no further call
    // carries this id and nothing else would ever end the dispatch.
    endDispatch(id);
    throw error;
  }
  if (isLastCall || entry === undefined || entry.stopped) {
    endDispatch(id);
  }
  return undefined;
}

/**
 * Ends one dispatch, on whichever of its three endings came first.
 *
 * The clean-up before the drop is the standard's own last dispatch step:
 * `eventPhase` back to `NONE` and `currentTarget` to null. It is observable,
 * because a listener may keep the event past the walk — a worklet closing
 * over it, an author stashing it — and without this the object it kept would
 * still name whichever node the walk happened to stop on. Only `target`
 * survives, as the standard leaves it.
 *
 * @param {number} id
 * @returns {undefined}
 */
function endDispatch(id) {
  const entry = dispatches.get(id);
  if (entry !== undefined) {
    entry.event.eventPhase = NONE;
    entry.event.currentTarget = null;
    dispatches.delete(id);
  }
  return undefined;
}

/**
 * Runs one filed `__AddEvent` handler, or does nothing when its kind is not
 * deliverable here.
 *
 * `runWorklet` is read off `globalThis` per delivery rather than captured:
 * it belongs to the card's own bundled worklet runtime, which installs it
 * long after this file runs, and a card with no main-thread handler never
 * installs it at all.
 *
 * @param {unknown} handler
 * @param {DispatchedEvent} event
 * @returns {undefined}
 */
function runEventHandler(handler, event) {
  if (typeof handler !== "object" || handler === null) {
    // A string: a background-thread handler name, with no background realm
    // to publish it to.
    return undefined;
  }
  const worklet = /** @type {Record<string, unknown>} */ (handler);
  if (worklet["type"] !== "worklet") {
    return undefined;
  }
  const runWorklet = globalThis.runWorklet;
  if (typeof runWorklet === "function") {
    runWorklet(worklet["value"], [event]);
  }
  return undefined;
}

/**
 * One node's listeners for one dispatch.
 *
 * Returns the dispatch's entry, or `undefined` when this node contributed
 * nothing and none was needed.
 *
 * @param {number} id
 * @param {number} node
 * @param {number} targetNodeId
 * @param {number} phase
 * @param {string} name
 * @param {unknown} detailJson
 * @returns {Dispatch | undefined}
 */
function deliverEvent(id, node, targetNodeId, phase, name, detailJson) {
  // No handle, no listeners: they lived on the handle and went with it. The
  // host's index is maintained from here and forgets a node when its element
  // is dropped, so this is the window between a handle becoming unreachable
  // and its cleanup job running, not a node that was never named.
  const handle = handleOf(node);
  if (handle === undefined) {
    return dispatches.get(id);
  }
  const list = listenersOf(handle)?.get(name)?.[phase];
  const closures = list !== undefined && list.length > 0 ? list : undefined;
  const filed = handlersOf(handle)?.[STATIC].get(name);
  const handled =
    filed !== undefined && phaseOfType(filed.type) === phase
      ? filed
      : undefined;
  if (closures === undefined && handled === undefined) {
    return dispatches.get(id);
  }

  const entry = dispatchEntry(id, name, targetNodeId, detailJson);
  const event = entry.event;
  entry.immediate = false;
  event.eventPhase = eventPhaseOf(node, targetNodeId, phase);
  event.currentTarget = targetInfo(node);
  // Only across a shadow boundary, where retargeting hands this node a
  // different target than the last one saw. Rebuilding unconditionally would
  // spend a host call per node and break `event.target === event.target`
  // across steps.
  if (targetNodeId !== entry.targetNodeId) {
    entry.targetNodeId = targetNodeId;
    event.target = targetInfo(targetNodeId);
  }

  // The `__AddEvent` handler first, then the `__AddEventListener` closures,
  // which is web-core's per-node order.
  if (handled !== undefined) {
    // Before the handler rather than after it: a `catch` form ends the walk
    // because of what it is, so it has to end it even when its handler is a
    // background-thread name that nothing here can call.
    if (isCatchType(handled.type)) {
      event.stopPropagation();
    }
    runEventHandler(handled.handler, event);
  }

  // Skipped whole when the handler above stopped immediate propagation:
  // the rest of this node's registrations is exactly what that suppresses.
  if (closures !== undefined && !entry.immediate) {
    // A copy, so a callback that adds or removes listeners for this same
    // node and name changes what the *next* event sees, not this one — the
    // standard's rule, one level down from the path the host froze.
    for (const registration of closures.slice()) {
      if (registration.removed) {
        continue;
      }
      if (registration.once) {
        removeListener(handle, name, registration.callback, {
          capture: phase === CAPTURE,
        });
      }
      registration.callback(event);
      if (entry.immediate) {
        break;
      }
    }
  }
  return entry;
}

/**
 * @param {unknown} element
 * @param {unknown} eventName
 * @param {unknown} callback
 * @param {unknown} options
 * @returns {undefined}
 */
export function __AddEventListener(element, eventName, callback, options) {
  return addListener(
    /** @type {object} */ (element),
    eventName,
    callback,
    options,
  );
}

/**
 * @param {unknown} element
 * @param {unknown} eventName
 * @param {unknown} callback
 * @param {unknown} options
 * @returns {undefined}
 */
export function __RemoveEventListener(element, eventName, callback, options) {
  return removeListener(
    /** @type {object} */ (element),
    eventName,
    callback,
    options,
  );
}

/**
 * Native Lynx takes the event object here, and so does this: the object is
 * minted per delivery and carries the methods, so the PAPI form is the same
 * call by another name.
 *
 * @param {unknown} event
 * @returns {undefined}
 */
export function __StopPropagation(event) {
  /** @type {{ stopPropagation?: () => void }} */ (event)?.stopPropagation?.();
  return undefined;
}

/**
 * @param {unknown} event
 * @returns {undefined}
 */
export function __StopImmediatePropagation(event) {
  /** @type {{ stopImmediatePropagation?: () => void }} */ (event)
    ?.stopImmediatePropagation?.();
  return undefined;
}

/** @returns {undefined} */
export function __FlushElementTree() {
  flushElementTree();
  return undefined;
}

// The host calls this export once per node per pass. It is deliberately not
// part of the entry preamble's PAPI imports; it is the module-namespace return
// path from Rust into the realm.
export { eventListenerCallback as __BobcatDispatchEvent };
