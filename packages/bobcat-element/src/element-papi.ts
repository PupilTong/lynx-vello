import {
  attributeNames,
  callElementMethod,
  childElementIds,
  createDocument,
  createElement,
  createPage,
  dropElement,
  flushElementTree,
  getAttribute,
  getComputedStyleMap,
  insertBefore,
  listenerNameClosed,
  listenerNameOpened,
  parentNode,
  removeAttribute,
  removeElement,
  replaceElement,
  setAttribute,
  setInlineStyles,
  setInlineStyleProperty,
  supportsStyleProperty,
  queryElementIds,
  swapElement,
  tagName,
} from "bobcat-internal:host";
import type { NodeQueryRequest, QueryNode } from "bobcat:selector-query";
import { __BobcatPublishEvent } from "bobcat:runtime";

// The Lynx Element PAPI runtime.
//
// Preloaded as the `bobcat:element` ESM inside the QuickJS main-thread realm;
// its Rstest suite imports the same bytes. It reaches native code only through
// named exports of the native `bobcat-internal:host` ESM. Named exports are
// the MTS bindings, plus the one lifecycle export — the `Document` class the
// boot module constructs — which is no PAPI member; this module installs no
// Element-PAPI globals.
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
// | `__AddInlineStyle(element, property, value)` | native `setInlineStyleProperty`; CSS names, not numeric native IDs |
// | `__SetDataset` / `__GetDataset` / `__AddDataset` | typed per-element values in this realm |
// | `__SetCSSId(elements, cssId, entryName?)` | nothing — accepted and ignored |
// | `__SetAttribute(element, name, value)` | native `setAttribute` / `removeAttribute` exports; `update-list-info` instead drives the list callbacks over `childElementIds` / `insertBefore` / `removeElement` |
// | `__UpdateListCallbacks(list, ...)` | this runtime's own store |
// | `__AddEvent(element, type, name, handler)` | this runtime's own store |
// | `__GetEvent(element, name, type)` | this runtime's own store |
// | `__GetEvents(element)` | this runtime's own store |
// | `__SetEvents(element, events)` | this runtime's own store |
// | `__AddEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__RemoveEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__StopPropagation(event)` | the event object's own method |
// | `__StopImmediatePropagation(event)` | the event object's own method |
// | `__GetPageElement()` | the page handle `__CreatePage` minted |
// | `__QuerySelector(element, selector, params)` | native `queryElementIds` export + this runtime's handle index |
// | `__QuerySelectorAll(element, selector, params)` | native `queryElementIds` export + this runtime's handle index |
// | `__InvokeUIMethod(element, method, params, callback)` | native `callElementMethod` + `getAttribute` exports + this runtime's dataset store |
// | `__GetComputedStyleByKey(element, key)` | native `getComputedStyleMap` export |
// | `__FlushElementTree()` | native `flushElementTree` export |
//
// Everything else — `__CreateFrame`, `__DropElement` (absent from every
// web-core generation), `__AddClass`,
// component-info, config, template-part and animation members, the rest of
// tree querying (`__GetParent`, `__FirstElement`, `__LastElement`,
// `__NextElement`, `__ElementIsEqual`, `__GetAttributes`), and list cell
// recycling — is not implemented. A bundle that reaches for another member
// fails at the missing global, not silently.
// BTS SelectorQuery uses the internal `__BobcatQueryNodes` export below;
// it resolves through the document's existing selector engine.
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
// capture behave as `addEventListener`'s do.
//
// **The whole walk is this file's.** The host computes the event path while
// it holds the document, releases it, and makes one call:
// `__BobcatDispatchEvent(nodes, targets, name, bubbles, timestamp,
// detailKind, ...detailNumbers)`, where `nodes` is the path in target-first
// order as comma-joined decimal node ids and `targets` carries, position for
// position, the shadow-retargeted target of that step. Two strings because
// the boundary takes primitives and structured clones only, a clone can be
// minted by the realm alone, and a decimal id cannot contain the separator —
// the same encoding `childElementIds` uses. Releasing the document before the
// call is what lets a listener mutate the tree.
//
// Everything else is numbers, and the objects they describe are built here,
// because their shape is JavaScript's. `timestamp` is the event's own, in
// milliseconds on the view's timeline. `detailKind` says which shape the
// numbers behind it make, and is the only thing that branches: one kind per
// `detail` *shape*, never per event name, so one export carries every
// producer the host has.
//
// - `DETAIL_POSITION` — every routed input event. `x`/`y` are the device
//   position the `detail` reports; `deltaX`/`deltaY` are the wheel delta,
//   `undefined` for every event that has none, which is what keeps those two
//   keys out of the `detail` rather than `NaN` in it; then four numbers per
//   touch point — `identifier`, `x`, `y`, and a flag bitmask over
//   `touches`/`targetTouches`/`changedTouches` — which the four touch events
//   alone carry.
// - `DETAIL_SIZE` — an `<image>`'s `load`: the bitmap's intrinsic `width` and
//   `height`, in px.
// - `DETAIL_EMPTY` — an `<image>`'s `error`: no numbers, and a `detail` of
//   `{}`.
//
// From there this file runs the standard's dispatch over that path: the
// capture pass from the last entry to the first, the bubble pass from the
// first to the last, one event object for both, `eventPhase` derived per
// step from whether the step is its own target, and `currentTarget` rebuilt
// per step. One object for the whole dispatch is both the standard's model
// and web-core's, and it is what makes a property one listener writes
// visible to the next. When the dispatch ends — normally, or because a
// listener threw — the standard's last dispatch step runs: `eventPhase` back
// to `NONE` and `currentTarget` to null, so an event a listener kept does not
// go on naming the node the walk stopped on.
//
// **A non-bubbling event narrows two of the three passes and cancels the
// third.** The whole path is always sent, because the capture pass runs over
// all of it whether the event bubbles or not; `bubbles` is what decides the
// rest. The bubble pass runs on the at-target steps alone — the target, plus
// any shadow host standing in for it — and the `global-bindEvent` pass does
// not run at all. That is web-core's `common_event_handler`
// (`web-core/src/main_thread/client/element_apis/event_apis.rs:413-432`):
// capture over the full path unconditionally, then either the full path or
// `[path.first()]`, and `dispatch_global_bind_event` only `if is_bubble`. It
// is handed the event's own `bubbles`
// (`ts/client/mainthread/elementAPIs/WASMJSBinding.ts:254-258`), so nothing
// about it is per-event-name. Lynx's own `<image>` `load` and `error` are the
// events this carries today: web-core builds both with `bubbles: false`
// (`web-elements/src/elements/common/commonEventInitConfiguration.ts`).
//
// Both stop methods are pure local state now. `stopPropagation` ends the
// remaining steps and `stopImmediatePropagation` also skips the rest of the
// current node's registrations; neither crosses the boundary, because there
// is no longer a walk on the other side to end.
//
// What the host does learn is the *name* set, and only its global edges: the
// first handle anywhere to carry a registration for a name calls the native
// `listenerNameOpened`, and the last to give one up calls
// `listenerNameClosed`. The painting side routes against that set, so a name
// nothing listens for never becomes a dispatch. The count behind those edges
// is kept here, per name, because every registration kind lives here — two
// closure lists and four `__AddEvent` maps — and only this file can know when
// a name's first registration appears or its last disappears. A handle's own
// counted names travel with it into the `FinalizationRegistry`'s held record,
// so a collected element closes what it held open.
//
// # `__AddEvent`, the other registration form
//
// `__AddEventListener` files a callable under the standard's identity.
// `__AddEvent` files handlers under a Lynx dispatch form — `bindEvent`,
// `catchEvent`, `capture-bind`, `capture-catch`, `global-bindEvent`. It is
// the form ReactLynx's compiled output uses for every `bind*`/`catch*` prop,
// so it is the one a real card exercises.
//
// **Two handler kinds, filed apart.** A *string* is an opaque
// background-thread handler name; an *object* is a worklet. Each element
// holds one of each per event name, and a call of one kind never disturbs
// the other — a `main-thread:bindtap` and a `bindtap` on the same element
// both run. That is both references' shape: native Lynx keeps `static_events_`
// beside `lepus_events_` (`attribute_holder.h`), and web-core keeps a
// cross-thread-handler map beside a run-worklet map, which its `__AddEvent`
// writes to by the handler's type (`createElementAPI.ts`). Only a nullish
// handler clears, and it clears both kinds, as web-core's does.
//
// Within a kind the key is the event *name* alone, with the dispatch form
// carried inside the entry, which is native Lynx's `insert_or_assign`: filing
// `catchtap` over `bindtap` of the same kind replaces it, form included.
//
// The two registration forms share one delivery per step and one name count.
// The form supplies what the standard's identity does not carry: which pass
// to file in, and whether the walk ends after this node.
//
// What a handler *is* decides where it runs. A worklet runs through the
// card's own `runWorklet`. A string is published with a snapshot of the event
// through the MTS runtime. A `catch` form ends the walk before either
// kind is delivered, because ending it is the form's doing, not the
// handler's. Anything else non-nullish is ignored, neither filed nor clearing
// what the name held, which is web-core's behavior for it. Native Lynx would
// take a callable and file it as a Lepus handler; web-core has nowhere to run
// one, and matching web-core is the compatibility target.
//
// `global-bindEvent` is filed in its own slot and delivered in a pass of its
// own, after the two path passes. It is not a pass over the path: every
// element holding a global registration for the name is delivered to, in
// registration order, whatever path the event took and whether or not a
// `catch` ended the walk over it — web-core's `common_event_handler` calls
// `dispatch_global_bind_event` unconditionally. The ids to visit are kept in
// this file's own registry of (name, node ids), which is what web-core's
// `update_global_bind_events` keeps too. A global delivery therefore has no
// `eventPhase`: `NONE`, since no step of any path produced it.
//
// # What is deliberately absent
//
// The pieces of `__AddEventListener` that duplicate `__AddEvent`:
// `closure_type` selecting a background handler string, and `bind_type`
// selecting Lynx's `catch` forms. A card that wants either has `__AddEvent`.
//
// List cell *recycling*. The data protocol itself is here:
// `__CreateList`/`__UpdateListCallbacks` file the callbacks and
// `__SetAttribute(element, "update-list-info", …)` services them in a
// microtask over the host's `childElementIds`, `insertBefore` and
// `removeElement` (see [`updateListInfo`]). What is absent is the layer
// above it — a recycling pool, a virtualization window, and the
// `componentAtIndexes` batch callback, which is filed and never called, as
// in web-core.
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
// - **The document is not on this schedule at all.** `class Document`'s
//   constructor calls the native `createDocument`, which builds the document
//   from the configuration it is given plus the view's own resources, which
//   stay on the host side; a realm
//   gets exactly one, and a second construction is refused by the host
//   whichever module asks. The boot module's first statement constructs it and
//   an exported binding holds it for the realm's life, so nothing here
//   releases it and there is no member that could: the document goes when the
//   realm does, and the host frees it after the realm, not from a cleanup
//   job. That is the opposite of the element path above, where cards genuinely
//   unroot handles and a collection every 32 removals frees what they named.

declare global {
  /**
   * Installed on this realm by the card's own bundled worklet runtime, and only
   * once a card actually compiles a main-thread function. `__AddEvent` reads it
   * per delivery rather than capturing it, because this file runs first.
   */
  var runWorklet:
    | ((worklet: unknown, params: unknown[]) => void)
    | undefined;
}

const nodeIdSymbol = Symbol("nodeId");

/**
 * Which listener list a registration belongs to, and which pass runs it: the
 * bubble pass walks the path's steps in order, the capture pass in reverse.
 */
const BUBBLE = 0;
const CAPTURE = 1;

/**
 * The standard's `Event.eventPhase` values, which are not the pass numbers
 * above: at the target the two do not even correspond, since both passes
 * visit it. A step derives its phase from whether it is its own target.
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
 */
const listenersSymbol = Symbol("listeners");
interface Registration {
  callback: Function;
  once: boolean;
  removed: boolean;
}
type ListenerLists = Map<string, [Registration[], Registration[]]>;

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
 * Which of an element's two handler slots a `type` selects: the path forms
 * (`bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`) file in
 * `STATIC`, `global-bindEvent` in `GLOBAL`. Native Lynx splits the same way
 * — every setter in `AttributeHolder` tests `kGlobalBind` first and writes
 * `global_bind_events_` rather than the maps its path walk reads — and so
 * does web-core, whose `update_global_bind_events` keeps the same index of
 * the elements a global delivery has to visit that [`globalNodes`] is.
 *
 * Native folds both handler kinds into its one global map, where web-core
 * keeps them apart and its `dispatch_global_bind_event` reads both. web-core
 * is the compatibility target, so the kind split below applies to this slot
 * as much as to the other.
 */
const STATIC = 0;
const GLOBAL = 1;

/**
 * Which of a slot's two maps a *handler* selects, decided by what the handler
 * is rather than by its type: a string names a background-thread handler, an
 * object is a worklet. Filing one never disturbs the other, so an element can
 * carry both for one event name and both run.
 *
 * This is the pair native Lynx keeps as `static_events_` beside
 * `lepus_events_` (`AttributeHolder::SetStaticEvent` against
 * `SetWorkletEvent`), and web-core as its cross-thread map beside its
 * run-worklet map — two maps its `__AddEvent` chooses between by `typeof`,
 * clearing both only for a nullish handler.
 *
 * Each map is keyed by event *name* alone, with the dispatch form carried
 * inside the entry, which is native's `insert_or_assign` rather than
 * web-core's (name, type) pair: filing `catchtap` over `bindtap` of the same
 * kind replaces it, form included, and `__GetEvent` answers for the requested
 * form only.
 */
const STRING_HANDLER = 0;
const WORKLET_HANDLER = 1;

/**
 * Where a handle files its `__AddEvent` handlers: at most one per name in
 * each of the two kinds of each of the two slots. On the handle, for the
 * reason the listener lists are.
 */
const handlersSymbol = Symbol("handlers");
interface FiledHandler {
  type: string;
  name: string;
  handler: unknown;
}
/** One slot's two kinds, indexed by [`STRING_HANDLER`, `WORKLET_HANDLER`]. */
type HandlerKinds = [Map<string, FiledHandler>, Map<string, FiledHandler>];
/** Both slots, indexed by [`STATIC`, `GLOBAL`]. */
type HandlerMaps = [HandlerKinds, HandlerKinds];

/**
 * How many handles currently carry *any* registration for an event name —
 * a closure in either pass, or a handler in any of the four `__AddEvent`
 * maps.
 *
 * The host keeps no listener index at all now, only the set of names the
 * painting side routes against, and it learns that set through the two
 * global edges of this count: 0 to 1 opens a name, 1 to 0 closes it. The
 * count has to live here because every registration does. A handle is
 * counted once per name however many registrations it holds for it, which
 * is what makes the reconciliation below a per-(handle, name) decision
 * rather than a sum.
 */
const listenerNameCounts: Map<string, number> = new Map();

/**
 * The node ids holding a `global-bindEvent` handler of either kind, per
 * event name, in registration order — which is delivery order, since no
 * path orders a global registration.
 *
 * A `Set` keeps the position of its first insertion when a member is added
 * again while still present, which is exactly the rule wanted: re-filing a
 * handler on an element already registered must not move it behind the ones
 * that followed it. Ids rather than handles, so this registry cannot be what
 * keeps an element alive; a dead one is skipped at delivery and pruned by
 * the collection that took its handle.
 */
const globalNodes: Map<string, Set<number>> = new Map();

/**
 * The list callbacks `__CreateList` and `__UpdateListCallbacks` file.
 *
 * Their consumer is `__SetAttribute(element, "update-list-info", …)`, which
 * reads them out of the handle it is given — see [`updateListInfo`]. They
 * live on the handle because the list element is what a card names them for,
 * and they are retained rather than dropped because a callback dropped at
 * `__CreateList` time cannot be recovered later: the card hands each over
 * exactly once.
 *
 * **Read at service time, not at the call.** ReactLynx's
 * `ListUpdateInfoRecording.flush` writes the operations first and files the
 * callbacks immediately after, so the pair a given batch runs against is the
 * one in place when the microtask drains, never the one in place when the
 * attribute was written.
 */
const listCallbacksSymbol = Symbol("listCallbacks");
interface ListCallbacks {
  componentAtIndex: unknown;
  enqueueComponent: unknown;
  componentAtIndexes: unknown;
}
const datasetSymbol = Symbol("dataset");
const attributeValuesSymbol = Symbol("attributeValues");

function valuesOf(element: unknown, key: typeof datasetSymbol | typeof attributeValuesSymbol): Map<string, unknown> {
  nodeIdOf(element);
  const handle = element as Handle;
  return handle[key] ??= new Map();
}

// Native keeps typed attribute/dataset values separately from DOM strings.
// Copy containers at assignment, retaining local functions as local values;
// fields() filters function attributes and the BTS transport structure-clones
// the remaining result. Assigning an attribute is not a cross-realm operation.
function copyElementValue<T>(value: T, copies = new Map<object, unknown>()): T {
  if (value === null || typeof value !== "object") return value;
  if (copies.has(value)) return copies.get(value) as T;
  const result = Array.isArray(value) ? new Array(value.length) : {};
  copies.set(value, result);
  for (const key of Object.keys(value)) {
    Object.defineProperty(result, key, {value: copyElementValue(Reflect.get(value, key), copies),
      enumerable: true, writable: true, configurable: true});
  }
  return result as T;
}

/**
 * The handles of a handle's children: an unordered strong set, which is what
 * keeps a connected element's handle from being collected under it.
 *
 * Membership only. Order is the native tree's, and the six tree mutations
 * that maintain this set never touch it — mirroring it here would be a second
 * answer to a question the host already answers, and the two could disagree.
 *
 * Created on first use, because most elements are leaves.
 */
const ownedChildrenSymbol = Symbol("ownedChildren");
type OwnedChildren = Set<Handle>;

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
 * What a handle leaves behind for the collection that frees its element:
 * its node id, and the event names it is counted under in
 * [`listenerNameCounts`].
 *
 * A record rather than the bare id, because a name a dead handle held open
 * has to be closed and its entry in [`globalNodes`] pruned, and the cleanup
 * runs when the handle — and every set on it — is already unreachable. The
 * handle points at this record and the record never points back: a held
 * value that reached its target would keep it alive forever, and a
 * `FinalizationRegistry` is specified to reject exactly that.
 *
 * The `names` set is created on first registration. Most elements never get
 * one, so most handles pay this record and nothing more.
 */
const collectedSymbol = Symbol("collected");
interface Collected {
  readonly nodeId: number;
  names: Set<string> | undefined;
}

/**
 * A handle as this file sees it: the node id, plus whatever it has filed
 * on the handle under its own symbols.
 */
interface Handle {
  readonly [nodeIdSymbol]: number;
  [listenersSymbol]?: ListenerLists;
  [handlersSymbol]?: HandlerMaps;
  [collectedSymbol]?: Collected;
  [listCallbacksSymbol]?: ListCallbacks;
  [datasetSymbol]?: Map<string, unknown>;
  [attributeValuesSymbol]?: Map<string, unknown>;
  [ownedChildrenSymbol]?: OwnedChildren;
  [ownerSymbol]?: number | undefined;
}

function listenersOf(handle: Handle): ListenerLists | undefined {
  return handle[listenersSymbol];
}

function handlersOf(handle: Handle): HandlerMaps | undefined {
  return handle[handlersSymbol];
}

/**
 * The record this handle's collection will hand the cleanup, created on
 * first use. The page handle never reaches a registry, and gets one all the
 * same: it is where its counted names live.
 */
function collectedOf(handle: Handle): Collected {
  return handle[collectedSymbol] ??= {
    nodeId: nodeIdOf(handle),
    names: undefined,
  };
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
 */
const handlesByNodeId: Map<number, WeakRef<Handle>> = new Map();

/**
 * Frees the element of a handle that is gone, and gives up what that handle
 * had registered.
 *
 * The host frees that element alone: its element children are unlinked into
 * detached roots for their own handles, and only what no handle could name —
 * the text node a `raw-text` reflects — goes with it. The element cannot
 * still be connected, because a connected element's handle is held by its
 * parent's, up to the permanent page handle.
 *
 * Its registrations died with the handle, so the counts they held have to
 * come down here: an element that took the last registration for a name with
 * it closes that name, exactly as an explicit removal would have.
 */
const registry = new FinalizationRegistry(
  (collected: Collected) => {
    handlesByNodeId.delete(collected.nodeId);
    for (const name of collected.names ?? []) {
      closeName(name);
      forgetGlobalNode(name, collected.nodeId);
    }
    collected.names = undefined;
    dropElement(collected.nodeId);
  },
);

/**
 * The four Lynx page switches a document is built with, as the boot module
 * is written with them.
 *
 * The host writes them into the boot module as boolean literals; the boot
 * module reads `enableJSDataProcessor` out of the record for the MTS runtime
 * and hands the whole record to the constructor below.
 */
export interface PageConfig {
  /** Whether elements default to `display: linear`. */
  readonly defaultDisplayLinear: boolean;
  /** Whether elements default to visible overflow. */
  readonly defaultOverflowVisible: boolean;
  /** Whether author CSS selector matching is enabled. */
  readonly enableCssSelector: boolean;
  /** Whether page data reaches BTS without the MTS processor running. */
  readonly enableJSDataProcessor: boolean;
}

/**
 * The realm's document. Constructing one creates it, and it lives exactly as
 * long as the realm: nothing here releases it, and no registry watches it.
 *
 * The configuration is the constructor's one argument, so the realm decides
 * what the document is built as; the view's own resources — its metrics, its
 * fonts, its style pool and its author stylesheets — stay on the host side and
 * never reach this module. The call never waits: each author stylesheet is
 * mounted on the document by the host when its answer arrives.
 *
 * The boot module constructs exactly one, before it loads the card's entry,
 * and its exported binding is what holds the object. A card can reach this
 * class and construct a second one; the host refuses that, and the card's
 * boot fails with the host's message.
 */
export class Document {
  constructor(config: PageConfig) {
    createDocument(
      config.defaultDisplayLinear,
      config.defaultOverflowVisible,
      config.enableCssSelector,
      config.enableJSDataProcessor,
    );
  }

  get [Symbol.toStringTag](): string {
    return "Document";
  }
}

let pageHandle: Handle | undefined;

function createHandle(nodeId: number): Handle {
  const handle: Handle = { [nodeIdSymbol]: nodeId };
  handlesByNodeId.set(nodeId, new WeakRef(handle));
  registry.register(handle, collectedOf(handle), handle);
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
 */
function handleOf(nodeId: number): Handle | undefined {
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

function nodeIdOf(handle: unknown): number {
  return (handle as Handle)[nodeIdSymbol];
}

/**
 * The live handle that owns `handle`, or undefined when none does.
 *
 * Undefined is **not** "detached" — see the header. It covers two cases: no
 * owner was ever recorded, and the recorded owner's handle is gone. Only the
 * second is subtle, and what it means is that the owner is unreachable from
 * script and cannot be connected, so nothing this file files under it would
 * ever be read again.
 */
function ownerOf(handle: unknown): Handle | undefined {
  const owner = (handle as Handle)[ownerSymbol];
  if (owner === undefined) {
    return undefined;
  }
  return handleOf(owner);
}

/**
 * Takes `handle` out of its owner's child set, if it has one.
 */
function disown(handle: unknown): undefined {
  const slots = handle as Handle;
  const owner = slots[ownerSymbol];
  if (owner === undefined) {
    return undefined;
  }
  slots[ownerSymbol] = undefined;
  const parent = handleOf(owner);
  if (parent === undefined) {
    // Its handle is already gone, and its child set with it.
    return undefined;
  }
  parent[ownedChildrenSymbol]?.delete(slots);
  return undefined;
}

/**
 * Files `child` in `parent`'s child set, taking it out of whichever set held
 * it before. Called after the native mutation, so a call the host refuses
 * leaves this side exactly as the tree it failed to change.
 */
function adopt(parent: unknown, child: unknown): undefined {
  disown(child);
  const slots = parent as Handle;
  let owned = slots[ownedChildrenSymbol];
  if (owned === undefined) {
    owned = new Set();
    slots[ownedChildrenSymbol] = owned;
  }
  owned.add(child as Handle);
  (child as Handle)[ownerSymbol] = nodeIdOf(parent);
  return undefined;
}

/**
 * Files `child` under the handle for `parentNodeId`, the host's answer for
 * where the child now is.
 *
 * A parent with no live handle is one script has let go of, which cannot be
 * connected, and whose pending `dropElement` will unlink the child anyway:
 * there is no set to file it in and nothing is lost by not having one.
 */
function adoptUnder(parentNodeId: number, child: unknown): undefined {
  const parent = handleOf(parentNodeId);
  if (parent === undefined) {
    disown(child);
    return undefined;
  }
  adopt(parent, child);
  return undefined;
}

export function __CreatePage(
  componentID?: unknown,
  componentCSSID?: unknown,
): object {
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

export function __CreateElement(
  tag: unknown,
  parentComponentUniqueID: unknown,
): object {
  void parentComponentUniqueID;
  return createHandle(createElement(tag as string));
}

function createTag(tag: string, parentComponentUniqueID: unknown): object {
  void parentComponentUniqueID;
  return createHandle(createElement(tag));
}

export function __CreateWrapperElement(parentComponentUniqueID: unknown) {
  return createTag("wrapper", parentComponentUniqueID);
}

export function __CreateText(parentComponentUniqueID: unknown) {
  return createTag("text", parentComponentUniqueID);
}

export function __CreateImage(parentComponentUniqueID: unknown) {
  return createTag("image", parentComponentUniqueID);
}

export function __CreateView(parentComponentUniqueID: unknown) {
  return createTag("view", parentComponentUniqueID);
}

export function __CreateScrollView(parentComponentUniqueID: unknown) {
  return createTag("scroll-view", parentComponentUniqueID);
}

export function __CreateRawText(text: unknown): object {
  // The handle first, then the attribute. Nothing but a handle holds an
  // element, so a host call that throws in between would leave a node no
  // one could ever name or free — and `setAttribute` does throw, for a
  // value that is not a string.
  const handle = createHandle(createElement("raw-text"));
  setAttribute(nodeIdOf(handle), "text", text as string);
  return handle;
}

/**
 * List construction files the recycling callbacks the same way
 * `__UpdateListCallbacks` does; [`updateListInfo`] is what reads them.
 *
 * The rest parameter is what native declares as arguments 4 and 5: an
 * unused options object and the `componentAtIndexes` callback, which
 * ReactLynx passes here and not only through `__UpdateListCallbacks`. It
 * stays a rest parameter so the reported arity remains web-core's three.
 *
 * @param rest `[info, componentAtIndexes]`
 */
export function __CreateList(
  parentComponentUniqueID: unknown,
  componentAtIndex: unknown,
  enqueueComponent: unknown,
  ...rest: unknown[]
): object {
  void parentComponentUniqueID;
  const handle = createHandle(createElement("list"));
  handle[listCallbacksSymbol] = {
    componentAtIndex,
    enqueueComponent,
    componentAtIndexes: rest[1],
  };
  return handle;
}

export function __AppendElement(parent: unknown, child: unknown): object {
  insertBefore(nodeIdOf(parent), nodeIdOf(child), null);
  adopt(parent, child);
  return child as object;
}

export function __InsertElementBefore(
  parent: unknown,
  child: unknown,
  reference?: unknown,
): object {
  if (reference === child) {
    return child as object;
  }
  insertBefore(
    nodeIdOf(parent),
    nodeIdOf(child),
    reference === undefined || reference === null
      ? null
      : nodeIdOf(reference),
  );
  adopt(parent, child);
  return child as object;
}

export function __RemoveElement(parent: unknown, child: unknown): object {
  void parent;
  removeElement(nodeIdOf(child));
  disown(child);
  return child as object;
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
 */
export function __ReplaceElements(
  parent: unknown,
  newChildren: unknown,
  oldChildren?: unknown,
): undefined {
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
 */
export function __SwapElement(childA: unknown, childB: unknown): undefined {
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

export function __ReplaceElement(
  newElement: unknown,
  oldElement: unknown,
): undefined {
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
 */
function hyphenate(name: string): string {
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
 */
export function __SetClasses(
  element: unknown,
  classNames: unknown,
): undefined {
  const nodeId = nodeIdOf(element);
  if (classNames) {
    setAttribute(nodeId, "class", String(classNames));
  } else {
    removeAttribute(nodeId, "class");
  }
  return undefined;
}

export function __SetID(element: unknown, id: unknown): undefined {
  const nodeId = nodeIdOf(element);
  if (id) {
    setAttribute(nodeId, "id", String(id));
  } else {
    removeAttribute(nodeId, "id");
  }
  return undefined;
}

export function __GetID(element: unknown): string | null {
  return getAttribute(nodeIdOf(element), "id");
}

/**
 * The element's Lynx tag. web-core maps its HTML stand-in back
 * (`x-view` -> `view`); this runtime creates elements under the Lynx tag
 * itself, so the DOM's own local name is already the answer.
 */
export function __GetTag(element: unknown): string {
  return tagName(nodeIdOf(element));
}

/**
 * One attribute's value, or null when the element does not carry it.
 *
 * `__GetID` is this member with the name fixed, and both read the same
 * native export: an id is an attribute here, not a field beside them.
 */
export function __GetAttributeByName(
  element: unknown,
  name: unknown,
): string | null {
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
 */
export function __GetAttributeNames(element: unknown): string[] {
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
 */
export function __GetChildren(element: unknown): object[] {
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
 */
export function __GetElementUniqueID(element: unknown): number {
  if (!element) {
    return -1;
  }
  return nodeIdOf(element) ?? -1;
}

export function __SetDataset(element: unknown, dataset: unknown) {
  // Native SetDataSet merges keys; it does not replace or clear prior keys.
  const values = valuesOf(element, datasetSymbol);
  if (dataset !== null && typeof dataset === "object") {
    for (const [key, value] of Object.entries(copyElementValue(dataset))) values.set(key, value);
  }
}

export function __GetDataset(element: unknown) {
  return copyElementValue(Object.fromEntries(valuesOf(element, datasetSymbol)));
}

export function __AddDataset(element: unknown, key: string, value: unknown) {
  if (typeof key !== "string") throw new TypeError("dataset key must be a string");
  valuesOf(element, datasetSymbol).set(key, copyElementValue(value));
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
 */
function splitRecord(record: string): string[] {
  const fields: string[] = [];
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
 */
function styleField(text: string): string {
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
 */
export function __SetInlineStyles(element: unknown, value: unknown): undefined {
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
 * Mutates one named CSS property without replacing the remaining block.
 * Empty/nullish values remove it; Stylo owns parsing and invalid-value handling.
 */
export function __AddInlineStyle(element: unknown, key: unknown, value: unknown) {
  if (typeof key === "number") throw new TypeError("numeric Lynx CSS property IDs are not supported");
  setInlineStyleProperty(nodeIdOf(element), String(key), value == null ? "" : String(value));
}

/**
 * One property's resolved value as text, or the empty string.
 *
 * The key crosses verbatim, which is web-core's contract — it is
 * `getComputedStyle(element).getPropertyValue(key)`, and CSSOM takes CSS
 * names, not IDL ones. So `margin-top` answers and `marginTop` is empty, and
 * so is a shorthand (the host reports longhands and custom properties only)
 * and anything the style system does not know. A non-string key is empty for
 * the same reason: no property is named by one.
 *
 * Resolved rather than computed, as CSSOM's `getComputedStyle` is: `width`,
 * `height`, `margin-*` and `padding-*` report the used px of the last layout
 * pass when the element has a box. Nothing here flushes — an element that
 * has never been through a flush reports nothing at all.
 */
export function __GetComputedStyleByKey(element: unknown, key: unknown): string {
  if (typeof key !== "string") {
    return "";
  }
  // Name then value, so the value is the second field; an unknown name
  // answers an empty record and falls through to the empty string.
  return splitRecord(getComputedStyleMap(nodeIdOf(element), key, 1))[1] ?? "";
}

/**
 * One computed CSS value, as CSS Typed OM's base class and nothing more.
 *
 * The host serializes every value it reports, so this holds that text and
 * answers it from `toString()`. None of the numeric subclasses
 * (`CSSUnitValue`, `CSSKeywordValue`, ...) and no `CSSStyleValue.parse`: a
 * typed model would have to be minted from text that was computed from a
 * typed model on the other side of the boundary, and nothing in this runtime
 * consumes one.
 */
export class CSSStyleValue {
  readonly #text: string;

  constructor(text: string) {
    this.#text = text;
  }

  toString(): string {
    return this.#text;
  }
}

/** The read-only `StylePropertyMapReadOnly` shape [`__BobcatComputedStyleMap`] answers. */
export interface ComputedStyleMap {
  readonly size: number;
  get(name: unknown): CSSStyleValue | undefined;
  getAll(name: unknown): CSSStyleValue[];
  has(name: unknown): boolean;
  entries(): IterableIterator<[string, CSSStyleValue[]]>;
  keys(): IterableIterator<string>;
  values(): IterableIterator<CSSStyleValue[]>;
  forEach(
    callback: (
      values: CSSStyleValue[],
      name: string,
      map: ComputedStyleMap,
    ) => void,
    thisArg?: unknown,
  ): void;
  [Symbol.iterator](): IterableIterator<[string, CSSStyleValue[]]>;
}

/**
 * The element's whole computed style as a CSS Typed OM
 * `StylePropertyMapReadOnly`: every author-facing longhand in code-point
 * order, then every custom property it carries, each mapped to a one-element
 * `CSSStyleValue` list.
 *
 * Not a PAPI member — neither reference has one — but the object
 * `Element.computedStyleMap()` answers, reached by name from the realm the
 * way `__BobcatQueryNodes` is.
 *
 * A **snapshot**, built per call out of one host answer. The standard's map
 * is `[SameObject]` and live, tracking the element's style as it changes;
 * reproducing that would mean a call per lookup and a lifetime for the map,
 * so this file takes the copy instead. Computed values, not resolved ones:
 * `__GetComputedStyleByKey` is the CSSOM side and this is the Typed OM side,
 * so a `width: 50%` is `50%` here and used px there. Nothing here flushes,
 * so an element that has never been through one answers an empty map — in
 * which every non-custom lookup throws, because to this map no such
 * property exists.
 */
export function __BobcatComputedStyleMap(element: unknown): ComputedStyleMap {
  const fields = splitRecord(getComputedStyleMap(nodeIdOf(element), "", 0));
  const properties = new Map<string, CSSStyleValue[]>();
  for (let field = 0; field + 1 < fields.length; field += 2) {
    properties.set(fields[field]!, [new CSSStyleValue(fields[field + 1]!)]);
  }
  /**
   * The spec's lookup: ASCII-lowercase the name, and throw a `TypeError` for
   * a name that is not a valid property. Every author-facing longhand is in
   * the map, so absence *is* invalidity — except for a custom property,
   * which is valid whether or not the element carries it and is therefore
   * simply missing.
   */
  const lookup = (name: unknown): CSSStyleValue[] | undefined => {
    const property = String(name).replace(
      /[A-Z]/g,
      (character) => character.toLowerCase(),
    );
    const values = properties.get(property);
    if (values === undefined && !property.startsWith("--")) {
      throw new TypeError(`${property} is not a valid property name`);
    }
    return values;
  };
  const map: ComputedStyleMap = {
    get size() {
      return properties.size;
    },
    get: (name) => lookup(name)?.[0],
    // A fresh list per call, as the standard's sequence is: what the caller
    // does to it is the caller's, not this snapshot's.
    getAll: (name) => [...(lookup(name) ?? [])],
    has: (name) => lookup(name) !== undefined,
    entries: () => properties.entries(),
    keys: () => properties.keys(),
    values: () => properties.values(),
    // WebIDL's maplike order: the value, then the key, then the map.
    forEach: (callback, thisArg) => {
      for (const [name, values] of properties) {
        callback.call(thisArg, values, name, map);
      }
    },
    [Symbol.iterator]: () => properties.entries(),
  };
  return Object.freeze(map);
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
 */
export function __SetCSSId(
  elements: unknown,
  cssId: unknown,
  entryName?: unknown,
): undefined {
  void elements;
  void cssId;
  void entryName;
  return undefined;
}

/**
 * One callback slot as it is called: web-core reaches both through `?.`, so
 * a nullish slot is a call that does not happen rather than a refusal.
 */
type ListCallback = ((...args: unknown[]) => unknown) | null | undefined;

/**
 * A list's element children right now, by node id.
 *
 * Read fresh at every step of the two loops in [`updateListInfo`] and never
 * cached, because both mutate the tree as they go and web-core reads
 * `element.children` — a *live* `HTMLCollection` — the same way. It matters
 * twice over: `removeAction` positions are indices into the pre-removal
 * child list, which is why the i-th removal looks at `position - i`; and
 * `componentAtIndex` appends the cell it built (ReactLynx's
 * `snapshot/list/list.ts:202`, `__AppendElement(list, root)`) before it
 * answers, so a snapshot taken before an insertion step is already stale by
 * the time the step needs it.
 */
function listChildren(listNodeId: number): number[] {
  const record = childElementIds(listNodeId);
  return record === "" ? [] : record.split(",").map(Number);
}

/**
 * The one live handle for a list child, which a list child always has.
 *
 * `__GetChildren`'s invariant, in the other place that depends on it: a
 * connected element's handle is held by its parent's, up to the permanent
 * page handle, and these children were appended by a `componentAtIndex` the
 * card is still holding. Undefined therefore means the ownership graph and
 * the tree disagree, and minting a second handle for a node whose first has
 * died would leave the host holding a node no handle names.
 */
function listChildHandle(nodeId: number, what: string): Handle {
  const handle = handleOf(nodeId);
  if (handle === undefined) {
    throw new Error(
      `__SetAttribute(update-list-info): ${what} ${nodeId}, which no live handle names`,
    );
  }
  return handle;
}

/**
 * `__SetAttribute(list, "update-list-info", operations)` — the list data
 * protocol, which is how a compiled ReactLynx `<list>` gets its cells.
 *
 * The card never builds a list's children itself. It records a batch of
 * insertions and removals against *indices*, writes the batch here, and
 * files a fresh `componentAtIndex`/`enqueueComponent` pair immediately
 * afterwards (`ListUpdateInfoRecording.flush`, ReactLynx's
 * `snapshot/list/listUpdateInfo.ts`). This member replays the batch through
 * that pair: `componentAtIndex` builds and appends the cell for an index and
 * answers its `unique_id`, `enqueueComponent` is told which cell is going
 * away, and the tree edits in between are ordinary `insertBefore` and
 * `removeElement` calls. web-core's
 * `client/mainthread/elementAPIs/createElementAPI.ts:461-499` is the
 * algorithm being matched, step for step.
 *
 * **It runs in a microtask, and that is load-bearing.** `flush()` writes the
 * operations *before* it files the callbacks, so a batch serviced
 * synchronously would run against the previous render's pair — or, for a
 * list's first batch, against no pair at all. Deferring to the end of the
 * current task is what makes the two calls one operation. The payload is
 * still read synchronously, as web-core's destructuring is; only the
 * callbacks are read late.
 *
 * `queueMicrotask` is a WHATWG global QuickJS does not have and no module
 * installs in the MTS realm (the BTS runtime's own `lynx.queueMicrotask` is
 * written over `Promise.resolve().then`), so that is what this uses. The
 * scheduling is the same — a promise job is a microtask — and the checkpoint
 * ending the entry drains it. The one difference is where a throw lands: a
 * `queueMicrotask` callback throws to the global error handler, a promise
 * job leaves an unhandled rejection, which the realm reports through the
 * runtime's rejection tracker.
 *
 * Two deliberate deviations from web-core:
 *
 * - A `null` or non-object payload is a no-op. web-core destructures the
 *   value at the call and so throws a `TypeError` naming neither the list
 *   nor the member; nothing a card does reaches that, and failing a whole
 *   render for it buys nothing.
 * - A sign `componentAtIndex` answers that no live handle names is a throw.
 *   web-core skips it (`if (childElement)`), because its handle index is a
 *   `WeakRef` table it treats as best-effort. Here it is the ownership graph
 *   and the tree disagreeing about an element this very call just built, and
 *   nothing after it in the batch can be trusted.
 *
 * `updateAction` is ignored: web-core's accepted payload type has only
 * `insertAction` and `removeAction`, and what an update carries is the
 * platform info of a cell that stayed where it was.
 */
function updateListInfo(list: unknown, operations: unknown): undefined {
  const handle = list as Handle;
  const listNodeId = nodeIdOf(handle);
  if (typeof operations !== "object" || operations === null) {
    return undefined;
  }
  const { insertAction, removeAction } = operations as {
    insertAction?: unknown;
    removeAction?: unknown;
  };
  void Promise.resolve().then(() => {
    const callbacks = handle[listCallbacksSymbol];
    const componentAtIndex = callbacks?.componentAtIndex as ListCallback;
    const enqueueComponent = callbacks?.enqueueComponent as ListCallback;
    if (Array.isArray(removeAction)) {
      // Ascending old indices, so the i-th removal has shifted i places.
      removeAction.forEach((position: unknown, removed: number) => {
        const childNodeId =
          listChildren(listNodeId)[Number(position) - removed];
        if (childNodeId === undefined) {
          return;
        }
        const child = listChildHandle(childNodeId, "the list holds child");
        enqueueComponent?.(handle, listNodeId, childNodeId);
        removeElement(childNodeId);
        disown(child);
      });
    }
    if (!Array.isArray(insertAction)) {
      return;
    }
    // Ascending positions, so each one is an index into the list as the
    // insertions before it have already left it.
    for (const action of insertAction) {
      const position = Number((action as { position?: unknown })?.position);
      const sign = componentAtIndex?.(
        handle,
        listNodeId,
        position,
        // The operation id, which no readback here consumes, and
        // `enableReuseNotification`, which needs the recycling pool this
        // engine does not have. web-core passes the same two constants.
        0,
        false,
      );
      if (typeof sign !== "number") {
        continue;
      }
      const child = listChildHandle(sign, "componentAtIndex answered");
      const reference = listChildren(listNodeId)[position];
      // `componentAtIndex` appended the cell, so it may already *be* the
      // child at that index — for an append-only batch it always is, and
      // moving it would be a host call that changed nothing.
      if (reference === sign) {
        continue;
      }
      insertBefore(listNodeId, sign, reference ?? null);
      adopt(handle, child);
    }
  });
  return undefined;
}

/**
 * `null`/`undefined` removes; anything else is stringified, which is what
 * web-core's `setElementPropertyOrAttribute` does for every name that is not
 * a live property of its HTML stand-in element. `id`, `class`, and `style`
 * reach their specialized DOM paths inside the native `setAttribute` export.
 * A separate typed copy serves the Lynx fields() API. Functions remain local;
 * cross-thread readback uses the Worker transport's structured-clone semantics.
 *
 * `update-list-info` is the one name that is not an attribute at all: it is
 * the list data protocol's one command, and it goes to [`updateListInfo`]
 * instead of being stringified onto the element.
 */
export function __SetAttribute(
  element: unknown,
  name: unknown,
  value: unknown,
): undefined {
  if (name === "update-list-info") {
    return updateListInfo(element, value);
  }
  const nodeId = nodeIdOf(element);
  const values = valuesOf(element, attributeValuesSymbol);
  const key = String(name);
  if (value === null || value === undefined) {
    removeAttribute(nodeId, key);
    values.delete(key);
  } else {
    setAttribute(nodeId, key, String(value));
    values.set(key, copyElementValue(value));
  }
  return undefined;
}

/** One UI method's answer: the status shape every `invoke` path reports. */
interface InvokeResult {
  code: number;
  data: unknown;
}

/**
 * Runs one UI method on one element and shapes its answer.
 *
 * The host dispatches by name and answers text, or null for a name it has no
 * method for — which is code 3, `METHOD_NOT_FOUND`, web-core's code rather
 * than native's generic 1. `boundingClientRect`, the one method there is,
 * answers four numbers: `left`, `top`, `width`, `height`. `right` and
 * `bottom` are derived here rather than sent, because they are sums.
 *
 * `id` and `dataset` ride along as native does (web-core reports the id
 * only): `id` is the attribute, empty when the element carries none, the way
 * `element.id` is, and `dataset` is the copy `__GetDataset` hands out, so
 * what the caller receives is the caller's own.
 *
 * The rect is the last completed layout pass's, in viewport CSS px with
 * ancestor scroll offsets applied and transforms ignored — native's own
 * conversion has no transform support either. Nothing here flushes: a job
 * that mutates and measures sees the pre-mutation geometry until it calls
 * `__FlushElementTree`, which is also the order ReactLynx's
 * `Element.invoke` uses.
 */
function invokeUIMethod(handle: Handle, method: string): InvokeResult {
  const answer = callElementMethod(nodeIdOf(handle), method);
  if (answer === null) {
    return { code: 3, data: undefined };
  }
  const [left = 0, top = 0, width = 0, height = 0] = answer
    .split(",")
    .map(Number);
  return {
    code: 0,
    data: {
      id: __GetID(handle) ?? "",
      dataset: __GetDataset(handle),
      left,
      top,
      right: left + width,
      bottom: top + height,
      width,
      height,
    },
  };
}

/**
 * The MTS UI-method call. `params` is accepted and ignored: no method this
 * engine has reads one, and the options `boundingClientRect` takes on the
 * platforms — `relativeTo`, `androidEnableTransformProps`,
 * `iOSEnableAnimationProps` — each name behavior this engine does not have.
 *
 * The callback runs synchronously inside the call, exactly once, as
 * web-core's does: the answer is already in hand when the host returns, and
 * a card that measures and then acts in the same job depends on it.
 */
export function __InvokeUIMethod(
  element: unknown,
  method: unknown,
  params: unknown,
  callback: (result: InvokeResult) => void,
): undefined {
  void params;
  callback(invokeUIMethod(element as Handle, String(method)));
  return undefined;
}

function nodeFields(element: Handle, fields: string[]): QueryNode {
  const id = nodeIdOf(element);
  const values = valuesOf(element, attributeValuesSymbol);
  const result: QueryNode = {};
  for (const field of fields) {
    switch (field) {
      case "id": result['id'] = getAttribute(id, "id") ?? ""; break;
      case "tag": result['tag'] = tagName(id); break;
      case "unique_id": result['unique_id'] = id; break;
      case "name": result['name'] = values.get("name") ?? getAttribute(id, "name") ?? ""; break;
      case "class": result['class'] = (getAttribute(id, "class") ?? "").split(/\s+/).filter(Boolean); break;
      case "dataset": case "dataSet": result[field] = __GetDataset(element); break;
      case "index": {
        const parent = parentNode(id);
        const hasElementParent = parent !== null && handleOf(parent) !== undefined;
        const siblings = hasElementParent ? childElementIds(parent).split(",").map(Number) : [];
        result['index'] = hasElementParent ? siblings.indexOf(id) : 0;
        break;
      }
      case "attribute": {
        const entries = splitRecord(attributeNames(id))
          .filter(key => key !== "id" && key !== "class" && key !== "style" && !key.startsWith("data-"))
          .map(key => [key, values.has(key) ? values.get(key) : getAttribute(id, key)])
          .filter(([, value]) => value != null && typeof value !== "function");
        result['attribute'] = Object.fromEntries(entries);
        break;
      }
    }
  }
  return result;
}

export function __BobcatQueryNodes(request: NodeQueryRequest) {
  const {operation, token, params} = request;
  const root = token.root_unique_id !== undefined ? handleOf(token.root_unique_id)
    : token.component_id ? undefined : pageHandle;
  let code = 0;
  let elements: Handle[] = [];
  if (!root) code = 2;
  else if (typeof token.identifier !== "string") throw new TypeError("node identifier must be a string");
  else if (token.type === 2) {
    // Native unique-ID lookup is global once the specified root exists.
    const selected = /^-?\d+$/.test(token.identifier) ? handleOf(Number(token.identifier)) : undefined;
    if (selected) elements = [selected];
  } else if (token.type === 0 || token.type === 1) {
    if (token.type === 0 && token.identifier === "") elements = [root];
    else if (token.type === 1) {
      // Compare the legacy ref attribute without interpolating author text
      // into selector syntax. Modern React refs already use a CSS token.
      const walk = [root];
      while (walk.length) {
        const element = walk.pop()!;
        if (getAttribute(nodeIdOf(element), "react-ref") === token.identifier) {
          elements.push(element);
          if (token.first_only) break;
        }
        walk.push(...(__GetChildren(element) as Handle[]).reverse());
      }
    } else {
      try {
        // Inclusive: SelectorQuery's scope includes the root it is given.
        const ids = queryElementIds(nodeIdOf(root), token.identifier, token.first_only ? 1 : 0, 1);
        elements = ids ? ids.split(",").map(id => {
          const element = handleOf(Number(id));
          if (!element) throw new Error("a queried live node has no handle");
          return element;
        }) : [];
      } catch (error) {
        // Selector parsing is the only normal failure from this validated root.
        if (error instanceof Error && error.message.includes("not a valid selector")) code = 5;
        else throw error;
      }
    }
  } else code = 5;
  if (!code && !elements.length) code = 2;
  const status = {code, data: code === 0 ? "success"
    : code === 5 ? `selector '${token.identifier}' not supported`
    : !root ? `root node not found with identifier = ${token.identifier}`
    : `no node found for selector '${token.identifier}'`};
  if (operation === "invoke") {
    // The reply is the bare status, not the `{status, data}` pair the node
    // operations answer with; the BTS facade reads `code` and `data` off it.
    if (code) return status;
    return invokeUIMethod(elements[0]!, String((params as { method: unknown }).method));
  }
  if (operation === "setNativeProps") {
    if (code || params === null || typeof params !== "object" || Array.isArray(params)) return;
    for (const element of elements) {
      for (const [name, value] of Object.entries(params)) {
        if (supportsStyleProperty(name)) __AddInlineStyle(element, name, value);
        else __SetAttribute(element, name, value);
      }
    }
    __FlushElementTree();
    return;
  }
  if (code) return {status, data: token.first_only ? null : []};
  const data = elements.map(element => {
    if (operation === "fields") return nodeFields(element, params as string[]);
    if (operation !== "path") throw new Error(`unknown node query ${operation}`);
    const path: QueryNode[] = [];
    let ancestor: Handle | undefined = element;
    while (ancestor) {
      path.push(nodeFields(ancestor, ["tag", "id", "dataSet", "index", "class"]));
      const parent = parentNode(nodeIdOf(ancestor));
      ancestor = parent === null ? undefined : handleOf(parent);
    }
    return path;
  });
  return {status, data: token.first_only ? data[0] : data};
}

/**
 * The pass a `__AddEvent` type is delivered in. Lynx's four path-walking
 * forms collapse onto the two passes over the path: the `capture-` pair is
 * the capture pass, `bindEvent`/`catchEvent` the bubble pass.
 * `global-bindEvent` is not one of them and never reaches this — it is filed
 * in the `GLOBAL` slot, whose whole content is the pass after both.
 */
function phaseOfType(type: string): 0 | 1 {
  return type === CAPTURE_BIND || type === CAPTURE_CATCH ? CAPTURE : BUBBLE;
}

/**
 * Whether a type ends the walk after its node. Both `catch` forms do, and
 * they do it because of what they are: native Lynx decides from the
 * registration's existence, not from what its handler did or whether one
 * ran at all.
 */
function isCatchType(type: string): boolean {
  return type === CATCH_EVENT || type === CAPTURE_CATCH;
}

/**
 * One element's four handler maps — two kinds in each of two slots — created
 * on demand.
 */
function handlersFor(handle: Handle): HandlerMaps {
  let maps = handlersOf(handle);
  if (maps === undefined) {
    maps = [[new Map(), new Map()], [new Map(), new Map()]];
    handle[handlersSymbol] = maps;
  }
  return maps;
}

/**
 * The handler of one kind filed for one name in one slot, if any.
 */
function filedHandler(
  handle: Handle,
  slot: 0 | 1,
  kind: 0 | 1,
  name: string,
): FiledHandler | undefined {
  return handlersOf(handle)?.[slot][kind].get(name);
}

/**
 * Opens a name with the host when this is the first handle to want it.
 */
function openName(name: string): undefined {
  const count = listenerNameCounts.get(name) ?? 0;
  listenerNameCounts.set(name, count + 1);
  if (count === 0) {
    listenerNameOpened(name);
  }
  return undefined;
}

/**
 * The reverse, called once per handle that was counted under `name`.
 */
function closeName(name: string): undefined {
  const count = listenerNameCounts.get(name) ?? 0;
  if (count > 1) {
    listenerNameCounts.set(name, count - 1);
    return undefined;
  }
  listenerNameCounts.delete(name);
  listenerNameClosed(name);
  return undefined;
}

/**
 * Takes one node out of a name's global-bind registry, dropping the name's
 * entry with its last member so the registry holds only names something is
 * registered for.
 */
function forgetGlobalNode(name: string, nodeId: number): undefined {
  const nodes = globalNodes.get(name);
  if (nodes === undefined) {
    return undefined;
  }
  nodes.delete(nodeId);
  if (nodes.size === 0) {
    globalNodes.delete(name);
  }
  return undefined;
}

/**
 * Whether anything at all on this handle is registered for `name`: a closure
 * in either pass, or a handler in any of the four `__AddEvent` maps.
 */
function hasRegistration(handle: Handle, name: string): boolean {
  const lists = listenersOf(handle)?.get(name);
  if (lists !== undefined && (lists[BUBBLE].length > 0 || lists[CAPTURE].length > 0)) {
    return true;
  }
  const maps = handlersOf(handle);
  if (maps === undefined) {
    return false;
  }
  return maps.some((slot) => slot.some((kind) => kind.has(name)));
}

/**
 * Reconciles the two realm-wide registries for one (handle, name) against
 * everything registered on that handle now. Every registration change calls
 * it, and it is where a name edge reaches the host.
 *
 * A handle is counted once per name however many registrations it holds, so
 * the decision is a membership test rather than a sum, and the transitions
 * are the only thing that crosses: the first handle to want a name opens it,
 * the last to give it up closes it.
 */
function reconcile(handle: Handle, name: string): undefined {
  const collected = collectedOf(handle);
  const wanted = hasRegistration(handle, name);
  if (wanted !== (collected.names?.has(name) ?? false)) {
    if (wanted) {
      (collected.names ??= new Set()).add(name);
      openName(name);
    } else {
      collected.names?.delete(name);
      closeName(name);
    }
  }
  const maps = handlersOf(handle);
  const global = maps !== undefined &&
    (maps[GLOBAL][STRING_HANDLER].has(name) ||
      maps[GLOBAL][WORKLET_HANDLER].has(name));
  if (global) {
    let nodes = globalNodes.get(name);
    if (nodes === undefined) {
      nodes = new Set();
      globalNodes.set(name, nodes);
    }
    // Re-adding a member a `Set` already holds leaves it where it was, which
    // is what keeps a re-filed handler in its original delivery position.
    nodes.add(nodeIdOf(handle));
  } else {
    forgetGlobalNode(name, nodeIdOf(handle));
  }
  return undefined;
}

/**
 * The listener lists for one element and event name, created on demand.
 */
function listsFor(
  handle: Handle,
  name: string,
): [Registration[], Registration[]] {
  let byName = listenersOf(handle);
  if (byName === undefined) {
    byName = new Map();
    handle[listenersSymbol] = byName;
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
 * A list going from empty to occupied can be what opens this event name
 * with the host, which is what makes the painting side route the event at
 * all. The telling goes through [`reconcile`], because an `__AddEvent`
 * handler counts under the same name and neither kind may close it while the
 * other still wants it.
 */
function addListener(
  handle: Handle,
  eventName: unknown,
  callback: unknown,
  options: unknown,
): undefined {
  if (typeof callback !== "function") {
    // web-core ignores a non-callable under the default closure type; a
    // string handler is a background-thread name, supported by __AddEvent.
    return undefined;
  }
  const name = String(eventName).toLowerCase();
  const settings =
    (options ?? undefined) as Record<string, unknown> | undefined;
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
  reconcile(handle, name);
  return undefined;
}

/**
 * `removeEventListener`. Capture is part of the identity, so a bubble-phase
 * removal leaves a capture registration of the same callback alone.
 */
function removeListener(
  handle: Handle,
  eventName: unknown,
  callback: unknown,
  options: unknown,
): undefined {
  const name = String(eventName).toLowerCase();
  const byName = listenersOf(handle);
  const lists = byName?.get(name);
  if (lists === undefined) {
    return undefined;
  }
  const settings =
    (options ?? undefined) as Record<string, unknown> | undefined;
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
  reconcile(handle, name);
  return undefined;
}

/**
 * `__AddEvent`'s registration, shared with `__SetEvents`.
 *
 * The identity is (element, slot, kind, name) — the slot being the one `type`
 * selects and the kind being what the handler *is* — so a second call with a
 * handler of the same kind replaces the first outright, type included. That
 * is `insert_or_assign` on native Lynx's event map, and it is what ReactLynx's
 * per-slot updater relies on: it rewrites the same binding on every render
 * rather than removing and re-adding it. A handler of the *other* kind is
 * left alone, which is what lets a `main-thread:bindtap` worklet and a
 * `bindtap` background handler coexist on one element and both run.
 *
 * A nullish handler is the removal, matching `FiberAddEvent`'s
 * empty-callback branch, and it removes both kinds — web-core's `__AddEvent`
 * clears its cross-thread map and its worklet map on that one branch.
 */
function addEvent(
  handle: Handle,
  eventType: unknown,
  eventName: unknown,
  handler: unknown,
): undefined {
  const type = String(eventType).toLowerCase();
  const name = String(eventName).toLowerCase();
  const slot = type === GLOBAL_BIND ? GLOBAL : STATIC;
  if (handler === null || handler === undefined) {
    const maps = handlersOf(handle);
    maps?.[slot][STRING_HANDLER].delete(name);
    maps?.[slot][WORKLET_HANDLER].delete(name);
  } else if (typeof handler === "string") {
    handlersFor(handle)[slot][STRING_HANDLER].set(name, { type, name, handler });
  } else if (typeof handler === "object") {
    handlersFor(handle)[slot][WORKLET_HANDLER].set(name, { type, name, handler });
  } else {
    // Neither a handler name nor a worklet. web-core's `__AddEvent` matches
    // none of its three branches on such a call and so does nothing at all
    // — it does not file, and it does not clear what the name already held.
    // A callable reaching here is native Lynx's Lepus handler; web-core has
    // no main-thread place to run one and this runtime does not invent one.
    return undefined;
  }
  reconcile(handle, name);
  return undefined;
}

/**
 * Files one handler for one element, event name and Lynx dispatch form.
 *
 * Two handler kinds are filed, in separate maps, so a call of one kind never
 * clears the other:
 *
 * - a **worklet** (`{ type: "worklet", value }`, what `main-thread:bind*`
 *   compiles to) runs here, through the `runWorklet` the card's own worklet
 *   runtime installs on this realm;
 * - a **string** is a background-thread handler *name*, published with an
 *   event snapshot through the MTS runtime. A `catch` form ends the walk
 *   here before the background handler receives it.
 *
 * A nullish handler clears both kinds. Anything else that is not nullish — a
 * callable above all — is ignored outright, which is what web-core does
 * with it.
 *
 * `global-bindEvent` is filed in its own slot and listed in [`globalNodes`].
 * Its pass is not over the event path: every global registration for the
 * name is delivered once both path passes have finished, in registration
 * order, whether or not a `catch` ended the walk over the path.
 */
export function __AddEvent(
  element: unknown,
  eventType: unknown,
  eventName: unknown,
  handler: unknown,
): undefined {
  return addEvent(element as Handle, eventType, eventName, handler);
}

/**
 * The *string* handler filed for one name, or undefined when the filed one
 * belongs to a different dispatch form — the type check `FiberGetEvent`
 * performs, which is only meaningful because the map is keyed by name alone.
 *
 * The string kind only: web-core's `get_event` reads its cross-thread map
 * and never its worklet one, so an element carrying only a worklet for a
 * name answers undefined here. `__GetEvents` is what reports both.
 *
 * The arguments are (name, type), the reverse of `__AddEvent`'s
 * (type, name). Both native Lynx and web-core order them this way.
 */
export function __GetEvent(
  element: unknown,
  eventName: unknown,
  eventType: unknown,
): unknown {
  const type = String(eventType).toLowerCase();
  const name = String(eventName).toLowerCase();
  const slot = type === GLOBAL_BIND ? GLOBAL : STATIC;
  const filed = handlersOf(element as Handle)?.[slot][STRING_HANDLER].get(name);
  if (filed === undefined || filed.type !== type) {
    return undefined;
  }
  return filed.handler;
}

/**
 * Every handler filed on one element: the path slot before the global one,
 * and within a name the string kind before the worklet kind, which is the
 * order web-core's `get_events` pushes its two maps in.
 *
 * The three references disagree on the shape: native Lynx returns a record
 * of name to array, web-core's WASM returns records spelled
 * `event_name`/`event_type`/`event_handler`, and the PAPI type both declare
 * says `{ type, name, function }[]`. The declared shape wins here, because
 * it is the only one that makes `__SetEvents(e, __GetEvents(e))` a faithful
 * round trip — the one property any caller could rely on.
 */
export function __GetEvents(
  element: unknown,
): { type: string; name: string; function: unknown }[] {
  const maps = handlersOf(element as Handle);
  if (maps === undefined) {
    return [];
  }
  const events: { type: string; name: string; function: unknown }[] = [];
  for (const slot of maps) {
    // A name is reported once, at the position its first filing gave it,
    // with both kinds together — the string one first.
    const names = new Set([
      ...slot[STRING_HANDLER].keys(),
      ...slot[WORKLET_HANDLER].keys(),
    ]);
    for (const name of names) {
      for (const kind of slot) {
        const filed = kind.get(name);
        if (filed !== undefined) {
          events.push({
            type: filed.type,
            name: filed.name,
            function: filed.handler,
          });
        }
      }
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
 */
export function __SetEvents(element: unknown, events: unknown): undefined {
  const handle = element as Handle;
  const maps = handlersOf(handle);
  if (maps !== undefined) {
    const names: string[] = [];
    for (const slot of maps) {
      for (const kind of slot) {
        names.push(...kind.keys());
        kind.clear();
      }
    }
    for (const name of names) {
      reconcile(handle, name);
    }
  }
  if (!Array.isArray(events)) {
    return undefined;
  }
  for (const event of events) {
    const record = event as Record<string, unknown>;
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
 * Storage only; [`updateListInfo`] is the reader, and it reads the handle
 * fresh in its microtask, so a call landing after the operations were
 * written is the one that serves them.
 */
export function __UpdateListCallbacks(
  list: unknown,
  componentAtIndex: unknown,
  enqueueComponent: unknown,
  componentAtIndexes: unknown,
): undefined {
  (list as Handle)[listCallbacksSymbol] = {
    componentAtIndex,
    enqueueComponent,
    componentAtIndexes,
  };
  return undefined;
}

/**
 * An event's `target`/`currentTarget`: web-core's descriptor shape,
 * `{ dataset, id, uid }` plus the live handle.
 *
 * `dataset` is [`datasetOf`]'s answer. Every worklet and every
 * `__AddEventListener` closure sees it, which is what web-core's
 * `generateTargetObject` gives them; publishing to the background thread
 * reads the same object.
 *
 * `elementRefptr` is the handle itself, which a main-thread callback is
 * entitled to — it is in the same realm and already holds one. Background
 * delivery drops it, since no handle crosses a thread.
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
 */
function targetInfo(
  nodeId: number,
): {
  dataset: Record<string, unknown>;
  id: string | null;
  uid: number;
  elementRefptr: object;
} {
  const handle = handleOf(nodeId);
  if (handle === undefined) {
    throw new Error(
      `no handle names element ${nodeId}: the element ownership graph and the tree disagree`,
    );
  }
  return {
    dataset: datasetOf(nodeId),
    id: getAttribute(nodeId, "id"),
    uid: nodeId,
    elementRefptr: handle,
  };
}

/**
 * One element's dataset as an event descriptor reports it: every `data-*`
 * attribute under DOMStringMap's camelCased name, with the typed values
 * `__SetDataset`/`__AddDataset` filed merged over them — native keeps those
 * apart from the DOM strings, and the typed value wins where both name the
 * same key.
 *
 * Read fresh at each use rather than kept, because web-core rebuilds the
 * whole descriptor per listener invocation and a listener that writes a
 * `data-*` attribute is expected to be seen by the steps after it. A node
 * whose handle is gone has no typed values left to merge; its attributes
 * still answer.
 */
function datasetOf(nodeId: number): Record<string, unknown> {
  const attributeDataset = Object.fromEntries(
    splitRecord(attributeNames(nodeId))
      .filter((name) => name.startsWith("data-") && !/[A-Z]/.test(name))
      .map((name): [string, string | null] => [
        name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()),
        getAttribute(nodeId, name),
      ]),
  );
  const handle = handleOf(nodeId);
  return {
    ...attributeDataset,
    ...(handle === undefined ? {} : __GetDataset(handle)),
  };
}

/**
 * The event object one dispatch hands every listener it reaches.
 *
 * One object for the whole dispatch, mutated as the passes advance, which is
 * both the standard's model and web-core's (`WASMJSBinding.ts` passes one
 * `LynxCrossThreadEvent` down its per-element loop): a property one listener
 * writes is there for the next. It is minted per dispatch and reachable from
 * nothing afterwards, so a listener that keeps one keeps only it.
 */
interface DispatchedEvent {
  type: string;
  eventPhase: number;
  target: ReturnType<typeof targetInfo>;
  currentTarget: ReturnType<typeof targetInfo> | null;
  detail: EventDetail;
  // Milliseconds on the view's timeline, which is this engine's time origin
  // — the semantics DOM's `Event.timeStamp` has. Native Lynx reports epoch
  // milliseconds instead; see docs/tracking/deviations.md.
  timestamp: number;
  // web-core gives every event one, and fills it for `transition*` and
  // `animation*` events alone — neither of which this engine dispatches, so
  // here it is always the empty object. One per dispatch, like the event.
  params: Record<string, unknown>;
  stopPropagation: () => void;
  stopImmediatePropagation: () => void;
  // The three lists only a touch event carries. Absent — not
  // `undefined`-valued — on every other event, because the transport carries
  // an `undefined`-valued key as one.
  touches?: TouchPointValues[];
  targetTouches?: TouchPointValues[];
  changedTouches?: TouchPointValues[];
}

/**
 * What one dispatched event reports about itself, beyond its name and target.
 *
 * Every key is optional because the host sends one detail *kind* per
 * dispatch and each kind writes its own: a routed input event writes the
 * device position in viewport CSS px and — for `wheel` alone — the scroll
 * delta; an `<image>`'s `load` writes the bitmap's intrinsic size; its
 * `error` writes nothing, and the detail is `{}`. A key a kind does not write
 * is absent, not `undefined`-valued: the transport carries an
 * `undefined`-valued key as one. A listener may write into this object; it is
 * minted per dispatch, like the event that carries it.
 */
interface EventDetail {
  x?: number;
  y?: number;
  deltaX?: number;
  deltaY?: number;
  width?: number;
  height?: number;
}

/**
 * The detail kinds the host discriminates with, mirroring its own `DETAIL_*`
 * constants (`main/runtime/lib.rs`). One per shape the numbers make, so
 * nothing here branches on the event's name.
 */
const DETAIL_POSITION = 0;
const DETAIL_SIZE = 1;
const DETAIL_EMPTY = 2;

/**
 * The `detail` object one kind's numbers make.
 *
 * `DETAIL_EMPTY` spends none and is web-core's `error` detail exactly.
 * `DETAIL_SIZE` spends two, an image `load`'s `naturalWidth`/`naturalHeight`.
 * `DETAIL_POSITION` spends two, then two more for a wheel delta that may be
 * absent; whatever follows those four is the touch numbers, which
 * [`touchLists`] reads.
 */
function detailOf(kind: number, numbers: unknown[]): EventDetail {
  if (kind === DETAIL_EMPTY) {
    return {};
  }
  if (kind === DETAIL_SIZE) {
    return { width: Number(numbers[0]), height: Number(numbers[1]) };
  }
  const detail: EventDetail = { x: Number(numbers[0]), y: Number(numbers[1]) };
  if (numbers[2] !== undefined) {
    detail.deltaX = Number(numbers[2]);
    detail.deltaY = Number(numbers[3]);
  }
  return detail;
}

/**
 * One entry of a touch event's lists.
 *
 * Every coordinate is the same viewport CSS pixel the raw pointer events
 * report, under all three of the standard's names: web-core's `x`/`y` are
 * lynx-view-local rather than native Lynx's element-local, and this runtime
 * has no separate screen space to report. `identifier` is the host's pointer
 * id. `screenX`/`screenY`, `radiusX`/`radiusY`, `force` and `rotationAngle`
 * are recorded gaps, not values withheld.
 */
interface TouchPointValues {
  identifier: number;
  x: number;
  y: number;
  pageX: number;
  pageY: number;
  clientX: number;
  clientY: number;
}

/** The three lists, as the host's flag bitmask names them. */
const TOUCH_ACTIVE = 1;
const TOUCH_TARGET = 2;
const TOUCH_CHANGED = 4;

/**
 * Sorts the host's touch numbers into the three lists, or nothing when the
 * event carries none.
 *
 * Four numbers per point, in order: `identifier`, `x`, `y`, flags. A point in
 * more than one list is one object in each, so
 * `event.touches[0] === event.targetTouches[0]` holds when both name the same
 * finger — nothing observes the identity, and one object per finger is what
 * the lists mean.
 */
function touchLists(numbers: unknown[]): {
  touches: TouchPointValues[];
  targetTouches: TouchPointValues[];
  changedTouches: TouchPointValues[];
} | undefined {
  if (numbers.length === 0) {
    return undefined;
  }
  const touches: TouchPointValues[] = [];
  const targetTouches: TouchPointValues[] = [];
  const changedTouches: TouchPointValues[] = [];
  for (let at = 0; at + 3 < numbers.length; at += 4) {
    const x = Number(numbers[at + 1]);
    const y = Number(numbers[at + 2]);
    const point: TouchPointValues = {
      identifier: Number(numbers[at]),
      x,
      y,
      pageX: x,
      pageY: y,
      clientX: x,
      clientY: y,
    };
    const flags = Number(numbers[at + 3]);
    if (flags & TOUCH_ACTIVE) {
      touches.push(point);
    }
    if (flags & TOUCH_TARGET) {
      targetTouches.push(point);
    }
    if (flags & TOUCH_CHANGED) {
      changedTouches.push(point);
    }
  }
  return { touches, targetTouches, changedTouches };
}

/**
 * The background event target: the descriptor minus the realm-local element
 * handle, which no thread but this one could name.
 *
 * Derived from the descriptor rather than recomputed from the element, so a
 * background handler is told exactly what the local listeners of the same
 * step were. An empty `id` attribute is reported as none, which is the value
 * a background handler reads for an element carrying no id at all.
 */
function backgroundTargetInfo(
  target: ReturnType<typeof targetInfo> | null,
): {
  dataset: Record<string, unknown>;
  id: string | null;
  uid: number;
} | null {
  if (target === null) {
    return null;
  }
  const { elementRefptr: _handle, ...values } = target;
  return { ...values, id: values.id || null };
}

/**
 * What is published in place of the event: values only.
 *
 * The two stop methods are destructured out rather than overwritten with
 * `undefined`, because the transport carries an `undefined`-valued key as one
 * rather than dropping it — and the handles in `target`/`currentTarget` are
 * replaced with the values that describe them. A touch event's three lists
 * are values already, so they cross with the rest. The copy itself is the
 * transport's, taken at send time: a new object here is only what keeps this
 * event's own later mutations, and the walk clearing `currentTarget`, out of
 * what was published.
 */
function backgroundEvent(event: DispatchedEvent): Record<string, unknown> {
  const {
    stopPropagation: _stop,
    stopImmediatePropagation: _stopImmediate,
    ...rest
  } = event;
  return {
    ...rest,
    target: backgroundTargetInfo(event.target),
    currentTarget: backgroundTargetInfo(event.currentTarget),
  };
}

/**
 * The standard's `eventPhase` for one step.
 *
 * A step whose target is itself is at-target — crossing a shadow boundary
 * sets the target to the node it crossed to, and nothing else makes the two
 * equal — and both passes visit it. Every other step takes its phase from
 * the pass it belongs to.
 */
function eventPhaseOf(node: number, target: unknown, phase: number): number {
  if (node === target) {
    return AT_TARGET;
  }
  return phase === CAPTURE ? CAPTURING_PHASE : BUBBLING_PHASE;
}

/**
 * Runs one filed `__AddEvent` handler, or does nothing when its kind is not
 * deliverable here.
 *
 * `runWorklet` is read off `globalThis` per delivery rather than captured:
 * it belongs to the card's own bundled worklet runtime, which installs it
 * long after this file runs, and a card with no main-thread handler never
 * installs it at all.
 */
function runEventHandler(handler: unknown, event: DispatchedEvent): undefined {
  if (typeof handler === "string") {
    // No component PAPI creates a non-page component_id yet. The accepted
    // parentComponentUniqueID creation argument is an element unique_id,
    // not a component_id, so these elements use the page event endpoint.
    __BobcatPublishEvent(undefined, handler, backgroundEvent(event));
    return undefined;
  }
  if (typeof handler !== "object" || handler === null) {
    return undefined;
  }
  const worklet = handler as Record<string, unknown>;
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
 * One step of the event path: the node visited, and the target the standard
 * reports there — the two differ only across a shadow boundary, where
 * retargeting hands the steps above it the node they can name.
 */
interface PathStep {
  node: number;
  target: number;
}

/**
 * The path the host computed, from its two comma-joined id strings.
 *
 * Target-first, root-last: the bubble order, which the capture pass reads
 * backwards. Two strings rather than one structure because the boundary
 * carries primitives and structured clones only, and a decimal id cannot
 * contain the separator — `childElementIds` encodes a list the same way.
 */
function pathSteps(pathIds: unknown, targetIds: unknown): PathStep[] {
  const path = String(pathIds ?? "");
  if (path === "") {
    return [];
  }
  const targets = String(targetIds ?? "").split(",");
  return path.split(",").map((field, index): PathStep => {
    const node = Number(field);
    const target = targets[index];
    return { node, target: target === undefined ? node : Number(target) };
  });
}

/**
 * The handler of one kind on a path step, or undefined when the one filed
 * belongs to the other pass.
 */
function handlerInPass(
  filed: FiledHandler | undefined,
  phase: 0 | 1,
): FiledHandler | undefined {
  return filed !== undefined && phaseOfType(filed.type) === phase
    ? filed
    : undefined;
}

/**
 * One whole dispatch, the single call the host makes per event.
 *
 * The host computed the path while it held the document and released it
 * before calling, which is what lets a listener mutate the tree. Everything
 * after that is this file's: the standard's capture pass from the path's
 * far end inwards, its bubble pass back out, then the `global-bindEvent`
 * pass, which is not over the path at all.
 *
 * `bubbles` narrows the last two and never the first: a non-bubbling event
 * binds on its at-target steps alone and runs no global pass. See the header.
 *
 * One event object serves all three, so a property one listener writes is
 * there for the next. Whatever ends the dispatch — the passes finishing, a
 * stop, or a listener throwing on its way out of this call — the standard's
 * last dispatch step runs: `eventPhase` back to `NONE` and `currentTarget`
 * to null, so an event a listener kept does not go on naming the node the
 * dispatch stopped on. `target` survives, as the standard leaves it.
 *
 * A step whose handle is gone is skipped: its registrations lived on that
 * handle and went with it, and this is the window between the handle
 * becoming unreachable and the cleanup that frees its element.
 *
 * The host passes the facts it owns as numbers and this file builds the
 * objects: the `detail` out of the kind it named and the numbers behind it,
 * and — for a position detail — the three touch lists out of the four numbers
 * per point that follow. An event with no touch points carries no such keys
 * at all.
 */
function dispatchEvent(
  pathIds: unknown,
  targetIds: unknown,
  eventName: unknown,
  bubbles: unknown,
  timestamp: unknown,
  detailKind: unknown,
  ...detailNumbers: unknown[]
): undefined {
  const steps = pathSteps(pathIds, targetIds);
  const first = steps[0];
  if (first === undefined) {
    return undefined;
  }
  const name = String(eventName).toLowerCase();
  let stopped = false;
  let immediate = false;
  let targetNodeId = first.target;
  // A reading the host could not take — a detached painter's, say — reports
  // the time origin rather than `NaN`.
  const stamp = Number(timestamp ?? 0);
  const kind = Number(detailKind);
  const detail = detailOf(kind, detailNumbers);
  const event: DispatchedEvent = {
    type: name,
    eventPhase: NONE,
    target: targetInfo(targetNodeId),
    currentTarget: null,
    detail,
    timestamp: Number.isNaN(stamp) ? 0 : stamp,
    params: {},
    stopPropagation: () => {
      stopped = true;
    },
    stopImmediatePropagation: () => {
      stopped = true;
      immediate = true;
    },
  };
  // Assigned rather than declared above, so an event that carries no touches
  // has no such keys at all — the transport would carry three
  // `undefined`-valued ones to a background handler otherwise. Only a
  // position detail can have any: its first four numbers are the position and
  // the wheel delta, and the points follow them.
  const lists = kind === DETAIL_POSITION
    ? touchLists(detailNumbers.slice(4))
    : undefined;
  if (lists !== undefined) {
    Object.assign(event, lists);
  }

  // Points the event at one delivery. `currentTarget` is rebuilt per step,
  // as web-core rebuilds it per listener invocation; `target` is kept while
  // it names the same node, so `event.target === event.target` holds across
  // a walk as it does in a browser, and only its `dataset` is read again —
  // a `data-*` attribute an earlier listener wrote has to be visible to the
  // steps after it. `id` is deliberately not refreshed; see the
  // cached-target note in docs/tracking/dom-events.md.
  const aim = (node: number, target: number, phase: number): undefined => {
    immediate = false;
    event.eventPhase = phase;
    event.currentTarget = targetInfo(node);
    if (target === targetNodeId) {
      event.target.dataset = datasetOf(targetNodeId);
    } else {
      targetNodeId = target;
      event.target = targetInfo(target);
    }
    return undefined;
  };

  // The string handler, then the worklet, then the `__AddEventListener`
  // closures, which is web-core's per-node order in both its path walk and
  // its global-bind delivery.
  const runPass = (ordered: PathStep[], phase: 0 | 1): undefined => {
    for (const step of ordered) {
      if (stopped) {
        return undefined;
      }
      const handle = handleOf(step.node);
      if (handle === undefined) {
        continue;
      }
      const published = handlerInPass(
        filedHandler(handle, STATIC, STRING_HANDLER, name),
        phase,
      );
      const worklet = handlerInPass(
        filedHandler(handle, STATIC, WORKLET_HANDLER, name),
        phase,
      );
      const list = listenersOf(handle)?.get(name)?.[phase];
      const closures = list !== undefined && list.length > 0 ? list : undefined;
      if (
        closures === undefined && published === undefined && worklet === undefined
      ) {
        continue;
      }
      aim(step.node, step.target, eventPhaseOf(step.node, step.target, phase));
      // Before either handler rather than after: a `catch` form ends the
      // walk because of what it is, so it has to end it even when its
      // handler is a background-thread name delivered asynchronously. Either
      // kind's form can be the catch.
      if (
        (published !== undefined && isCatchType(published.type)) ||
        (worklet !== undefined && isCatchType(worklet.type))
      ) {
        event.stopPropagation();
      }
      if (published !== undefined) {
        runEventHandler(published.handler, event);
      }
      if (worklet !== undefined && !immediate) {
        runEventHandler(worklet.handler, event);
      }
      // Skipped whole when a handler above stopped immediate propagation:
      // the rest of this node's registrations is exactly what that
      // suppresses.
      if (closures === undefined || immediate) {
        continue;
      }
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
        if (immediate) {
          break;
        }
      }
    }
    return undefined;
  };

  // The `global-bindEvent` registrations, in registration order. Not a pass
  // over the path — a global registration is delivered for every event of
  // its name whatever path it took — so it runs whether or not a `catch`
  // ended the walk over the path, which is what web-core's
  // `common_event_handler` does when it calls `dispatch_global_bind_event`
  // unconditionally. It runs no closures: `__AddEventListener` carries no
  // Lynx form and so never files globally. `eventPhase` is `NONE`, since no
  // step of any path produced the delivery, and `target` is the event's own.
  const runGlobalPass = (): undefined => {
    const registered = globalNodes.get(name);
    if (registered === undefined) {
      return undefined;
    }
    // A snapshot: a handler may file or clear a global registration, and the
    // set it would change is this one.
    for (const nodeId of [...registered]) {
      const handle = handleOf(nodeId);
      if (handle === undefined) {
        continue;
      }
      const maps = handlersOf(handle);
      const published = maps?.[GLOBAL][STRING_HANDLER].get(name);
      const worklet = maps?.[GLOBAL][WORKLET_HANDLER].get(name);
      if (published === undefined && worklet === undefined) {
        continue;
      }
      aim(nodeId, first.target, NONE);
      if (published !== undefined) {
        runEventHandler(published.handler, event);
      }
      if (worklet !== undefined && !immediate) {
        runEventHandler(worklet.handler, event);
      }
    }
    return undefined;
  };

  try {
    // The reversed copy is the capture order; a path is a handful of steps.
    runPass(steps.slice().reverse(), CAPTURE);
    // A non-bubbling event binds on its at-target steps alone — the target,
    // and any shadow host retargeting made stand in for it, which is the same
    // set the host would have sent had it built a non-bubbling path itself
    // (`dom`'s `event_steps` keeps exactly the at-target entries). web-core,
    // which has no retargeting to carry, narrows to the path's first entry.
    runPass(
      bubbles ? steps : steps.filter((step) => step.node === step.target),
      BUBBLE,
    );
    if (bubbles) {
      runGlobalPass();
    }
  } finally {
    event.eventPhase = NONE;
    event.currentTarget = null;
  }
  return undefined;
}

export function __AddEventListener(
  element: unknown,
  eventName: unknown,
  callback: unknown,
  options?: unknown,
): undefined {
  return addListener(element as Handle, eventName, callback, options);
}

export function __RemoveEventListener(
  element: unknown,
  eventName: unknown,
  callback: unknown,
  options?: unknown,
): undefined {
  return removeListener(element as Handle, eventName, callback, options);
}

/**
 * Native Lynx takes the event object here, and so does this: the object
 * carries the methods, so the PAPI form is the same call by another name.
 */
export function __StopPropagation(event: unknown): undefined {
  (event as { stopPropagation?: () => void })?.stopPropagation?.();
  return undefined;
}

export function __StopImmediatePropagation(event: unknown): undefined {
  (event as { stopImmediatePropagation?: () => void })
    ?.stopImmediatePropagation?.();
  return undefined;
}

/**
 * The page handle, or undefined before `__CreatePage` minted one.
 *
 * web-core's is `() => page`, the same binding its `__CreatePage` assigns, so
 * a card that asks before it has rendered gets nothing rather than an error.
 * ReactLynx's worklet runtime caches the answer and tests it for truth, which
 * is the contract that shape serves.
 */
export function __GetPageElement(): object | undefined {
  return pageHandle;
}

/**
 * The handles of `selector`'s matches under `element`, **excluding**
 * `element` itself.
 *
 * Root-exclusive because this is `Element.querySelector`'s scope, which is
 * what web-core implements these two members with. The `queryElementIds`
 * host member is also SelectorQuery's primitive, whose scope is Lynx's
 * inclusive one, so the exclusion is asked for explicitly — see
 * `__BobcatQueryNodes`, which asks for the other.
 *
 * A selector the host refuses throws its error, as `querySelector` throws a
 * `SyntaxError` for one. A match with no live handle throws for the reason
 * `__GetChildren` does: no second handle is ever minted for a node whose
 * first has died.
 */
function queryHandles(
  element: unknown,
  selector: unknown,
  firstOnly: 0 | 1,
): Handle[] {
  const ids = queryElementIds(
    nodeIdOf(element),
    String(selector),
    firstOnly,
    0,
  );
  if (ids === "") {
    return [];
  }
  return ids.split(",").map((field) => {
    const nodeId = Number(field);
    const handle = handleOf(nodeId);
    if (handle === undefined) {
      throw new Error(
        `a queried live node has no handle: element ${nodeId}`,
      );
    }
    return handle;
  });
}

/**
 * The first match of `selector` under `element`, or undefined.
 *
 * `params` — web-core's `{ onlyCurrentComponent? }` — is accepted and
 * ignored, as web-core ignores it: it scopes the query to the element's own
 * component, and no component PAPI creates a non-page component yet.
 */
export function __QuerySelector(
  element: unknown,
  selector: unknown,
  params?: unknown,
): object | undefined {
  void params;
  return queryHandles(element, selector, 1)[0];
}

/** Every match of `selector` under `element`, in tree order. */
export function __QuerySelectorAll(
  element: unknown,
  selector: unknown,
  params?: unknown,
): object[] {
  void params;
  return queryHandles(element, selector, 0);
}

export function __FlushElementTree(): undefined {
  flushElementTree();
  return undefined;
}

// The host calls this export once per dispatch, with the whole path. It is
// deliberately not part of the entry preamble's PAPI imports; it is the
// module-namespace return path from Rust into the realm.
export { dispatchEvent as __BobcatDispatchEvent };
