// @ts-check
// The Lynx Element PAPI runtime.
//
// Evaluated as a classic script inside the QuickJS main-thread realm before
// any bundle code runs (bobcat-core embeds it with include_str!); its Rstest
// suite imports the same bytes for side effects. No import/export syntax; it
// reaches the outside world only through `globalThis.bobcat` (the native
// object the host installs first) and the `__*` PAPI globals it assigns,
// mirroring web-core's flat `Object.assign(mtsRealm.globalWindow, ...)`.
//
// # Element PAPI scope
//
// | PAPI | Backed by |
// | --- | --- |
// | `__CreatePage(componentID, componentCSSID)` | `bobcat.createPage` |
// | `__CreateElement(tag, parentComponentUniqueID)` | `bobcat.createElement` |
// | `__CreateWrapperElement(parentComponentUniqueID)` | `bobcat.createElement` |
// | `__CreateText(parentComponentUniqueID)` | `bobcat.createElement` |
// | `__CreateImage(parentComponentUniqueID)` | `bobcat.createElement` |
// | `__CreateView(parentComponentUniqueID)` | `bobcat.createElement` |
// | `__CreateScrollView(parentComponentUniqueID)` | `bobcat.createElement` |
// | `__CreateRawText(text)` | `bobcat.createElement` + `bobcat.setAttribute` |
// | `__CreateList(parentComponentUniqueID, ...)` | `bobcat.createElement` + this runtime's own store |
// | `__AppendElement(parent, child)` | `bobcat.insertBefore` |
// | `__InsertElementBefore(parent, child, reference?)` | `bobcat.insertBefore` |
// | `__RemoveElement(parent, child)` | `bobcat.removeElement` |
// | `__ReplaceElement(newElement, oldElement)` | `bobcat.replaceElement` |
// | `__ReplaceElements(parent, newChildren, oldChildren?)` | `bobcat.parentNode` + `bobcat.insertBefore` + `bobcat.removeElement` |
// | `__SwapElement(childA, childB)` | `bobcat.swapElement` / `bobcat.replaceElement` |
// | `__SetClasses(element, classNames)` | `bobcat.setAttribute` / `bobcat.removeAttribute` |
// | `__SetID(element, id)` | `bobcat.setAttribute` / `bobcat.removeAttribute` |
// | `__GetID(element)` | `bobcat.getAttribute` |
// | `__GetTag(element)` | `bobcat.tagName` |
// | `__GetElementUniqueID(element)` | the handle's own node id |  // (= its Lynx unique id)
// | `__SetInlineStyles(element, value)` | `bobcat.setAttribute` / `bobcat.removeAttribute` / `bobcat.set_node_property` |
// | `__SetCSSId(elements, cssId, entryName?)` | nothing — accepted and ignored |
// | `__SetAttribute(element, name, value)` | `bobcat.setAttribute` / `bobcat.removeAttribute` |
// | `__UpdateListCallbacks(list, ...)` | this runtime's own store |
// | `__AddEvent(element, type, name, handler)` | this runtime's own store |
// | `__GetEvent(element, name, type)` | this runtime's own store |
// | `__GetEvents(element)` | this runtime's own store |
// | `__SetEvents(element, events)` | this runtime's own store |
// | `__AddEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__RemoveEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__StopPropagation(event)` | `bobcat.stopPropagation` |
// | `__StopImmediatePropagation(event)` | `bobcat.stopPropagation` + this runtime's own store |
// | `__FlushElementTree()` | `bobcat.flushElementTree` |
//
// Everything else — `__CreateFrame`, `__DropElement` (absent from every
// web-core generation), `__AddClass`, `__AddInlineStyle`, the dataset,
// component-info, config, template-part and animation members, tree and
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
// held here, keyed by the handle, and they die with it — nothing about a
// handler ever crosses into Rust.
//
// Registration is the standard's: identity is (element, name, callback,
// capture), a second add of those four is ignored outright, and `once` and
// capture behave as `addEventListener`'s do. What is not the standard's is
// that the host has to be told a node is worth visiting. A list going from
// empty to occupied calls `bobcat.enableEventListener(node, capture, name)`
// and back to empty calls `bobcat.disableEventListener(...)`, so the host
// keeps an index and skips every node that has nothing registered — the walk
// crosses the boundary only where a listener actually is.
//
// Dispatch is the host's walk and this file's per-node work. The host
// computes the whole event path while it holds the document, releases it, and
// then calls `bobcat.event_listener_callback(node, target, phase, name,
// detail, eventId, isLastCall)` once per node per pass. Releasing first is
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
// `bobcat.stopPropagation`, since the standard's `stopImmediatePropagation`
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
//   the life of the document, so a handle that outlives its element — one
//   the registry has not swept yet, an id a bundle stashed in a variable —
//   can only ever name something gone, never a later element that happens to
//   sit in the freed one's storage. Nothing in this file has to guard
//   against that case because it cannot arise.
// - Collection is the only release path — web-core's model, where a swept
//   WeakRef is what frees an element. Every non-page handle is registered
//   with a FinalizationRegistry whose cleanup calls `bobcat.dropElement`;
//   cleanup runs as a pending job at the host's job checkpoints, and never
//   at realm teardown, so the last committed tree survives the bootstrap
//   realm.
// - No misuse is validated here: a foreign handle resolves to undefined
//   and the call crashes at the native boundary.

(function () {
  "use strict";

  const bobcat = globalThis.bobcat;
  if (bobcat === null || typeof bobcat !== "object") {
    throw new Error(
      "the element PAPI runtime requires the native bobcat object on globalThis",
    );
  }

  // Captured once so later tampering with `globalThis.bobcat` cannot redirect
  // the PAPI's native calls.
  const native = {
    createPage: bobcat.createPage,
    createElement: bobcat.createElement,
    setAttribute: bobcat.setAttribute,
    setNodeProperty: bobcat.set_node_property,
    removeAttribute: bobcat.removeAttribute,
    getAttribute: bobcat.getAttribute,
    tagName: bobcat.tagName,
    parentNode: bobcat.parentNode,
    insertBefore: bobcat.insertBefore,
    removeElement: bobcat.removeElement,
    replaceElement: bobcat.replaceElement,
    swapElement: bobcat.swapElement,
    dropElement: bobcat.dropElement,
    flushElementTree: bobcat.flushElementTree,
    enableEventListener: bobcat.enableEventListener,
    disableEventListener: bobcat.disableEventListener,
    stopPropagation: bobcat.stopPropagation,
  };

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
   * One element's listeners, weak by handle so a registration can never keep
   * its element alive: the entry dies with the handle, which keeps collection
   * the only release path.
   *
   * Each element maps an event name to a pair of lists indexed by [`BUBBLE`,
   * `CAPTURE`]. A list holds `{ callback, once }` in registration order, which
   * is firing order.
   *
   * @typedef {{ callback: Function, once: boolean, removed: boolean }} Registration
   * @type {WeakMap<object, Map<string, [Registration[], Registration[]]>>}
   */
  const listeners = new WeakMap();

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
   * One element's `__AddEvent` handlers: at most one per name in each map.
   *
   * Weak by handle, for the reason the closure store is — an entry dies with
   * its handle, so a registration can never be what keeps an element alive.
   *
   * @typedef {{ type: string, name: string, handler: unknown }} FiledHandler
   * @type {WeakMap<object, [Map<string, FiledHandler>, Map<string, FiledHandler>]>}
   */
  const handlers = new WeakMap();

  /**
   * What this file has last told the host about a (handle, name, pass).
   *
   * The host indexes a plain set of `(node, capture)` per name, so
   * `disableEventListener` is unconditional — it cannot know that the other
   * registration kind still wants the node visited. Two kinds file into that
   * one index, `__AddEventListener` closures and one `__AddEvent` handler, so
   * the decision belongs here, taken from both, with only the transitions
   * crossing the boundary.
   *
   * @type {WeakMap<object, Map<string, [boolean, boolean]>>}
   */
  const indexed = new WeakMap();

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
   * @type {WeakMap<object, ListCallbacks>}
   */
  const listCallbacks = new WeakMap();

  /**
   * The reverse of a handle's `nodeIdSymbol`: dispatch arrives from the host
   * naming a `NodeId`, and the handler store is keyed by handle.
   *
   * Held weakly, and cleared in the same sweep that drops the element, so this
   * index cannot become the reference that keeps a handle — and through it an
   * element — alive. That is the same rule the handler store follows, and for
   * the same reason: collection stays the only release path.
   *
   * @type {Map<number, WeakRef<object>>}
   */
  const handlesByNodeId = new Map();

  const registry = new FinalizationRegistry(
    (/** @type {number} */ nodeId) => {
      handlesByNodeId.delete(nodeId);
      native.dropElement(nodeId);
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
   * job that will drop the element too.
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
   * @param {unknown} componentID
   * @param {unknown} componentCSSID
   * @returns {object}
   */
  function __CreatePage(componentID, componentCSSID) {
    void componentID;
    void componentCSSID;
    const nodeId = native.createPage();
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
  function __CreateElement(tag, parentComponentUniqueID) {
    void parentComponentUniqueID;
    return createHandle(native.createElement(/** @type {string} */ (tag)));
  }

  /**
   * The fixed-tag Snapshot constructors share one shape: accept and ignore
   * the parent-component id (kept as a named parameter so the reported arity
   * stays web-core's one), create the tag, mint the handle.
   *
   * @param {string} tag
   * @returns {(parentComponentUniqueID?: unknown) => object}
   */
  function tagCreator(tag) {
    return function (parentComponentUniqueID) {
      void parentComponentUniqueID;
      return createHandle(native.createElement(tag));
    };
  }

  const __CreateWrapperElement = tagCreator("wrapper");
  const __CreateText = tagCreator("text");
  const __CreateImage = tagCreator("image");
  const __CreateView = tagCreator("view");
  const __CreateScrollView = tagCreator("scroll-view");

  /**
   * @param {unknown} text
   * @returns {object}
   */
  function __CreateRawText(text) {
    const nodeId = native.createElement("raw-text");
    native.setAttribute(nodeId, "text", /** @type {string} */ (text));
    return createHandle(nodeId);
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
  function __CreateList(
    parentComponentUniqueID,
    componentAtIndex,
    enqueueComponent,
    ...rest
  ) {
    void parentComponentUniqueID;
    const handle = createHandle(native.createElement("list"));
    listCallbacks.set(handle, {
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
  function __AppendElement(parent, child) {
    native.insertBefore(nodeIdOf(parent), nodeIdOf(child), null);
    return /** @type {object} */ (child);
  }

  /**
   * @param {unknown} parent
   * @param {unknown} child
   * @param {unknown} reference
   * @returns {object}
   */
  function __InsertElementBefore(parent, child, reference) {
    if (reference === child) {
      return /** @type {object} */ (child);
    }
    native.insertBefore(
      nodeIdOf(parent),
      nodeIdOf(child),
      reference === undefined || reference === null
        ? null
        : nodeIdOf(reference),
    );
    return /** @type {object} */ (child);
  }

  /**
   * @param {unknown} parent
   * @param {unknown} child
   * @returns {object}
   */
  function __RemoveElement(parent, child) {
    void parent;
    native.removeElement(nodeIdOf(child));
    return /** @type {object} */ (child);
  }

  /**
   * web-core's algorithm: without old children this is a plain append; with
   * them, every old child after the first is detached and the first is
   * replaced in place — under its actual parent, a no-op when detached,
   * exactly `ChildNode.replaceWith`.
   *
   * @param {unknown} parent
   * @param {unknown} newChildren
   * @param {unknown} oldChildren
   * @returns {undefined}
   */
  function __ReplaceElements(parent, newChildren, oldChildren) {
    const news = Array.isArray(newChildren) ? newChildren : [newChildren];
    if (!oldChildren || (Array.isArray(oldChildren) && oldChildren.length === 0)) {
      const parentNodeId = nodeIdOf(parent);
      for (const child of news) {
        native.insertBefore(parentNodeId, nodeIdOf(child), null);
      }
      return undefined;
    }
    const olds = Array.isArray(oldChildren) ? oldChildren : [oldChildren];
    for (let index = 1; index < olds.length; index += 1) {
      native.removeElement(nodeIdOf(olds[index]));
    }
    const first = nodeIdOf(olds[0]);
    const actualParent = native.parentNode(first);
    if (actualParent === null) {
      return undefined;
    }
    for (const child of news) {
      native.insertBefore(actualParent, nodeIdOf(child), first);
    }
    native.removeElement(first);
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
  function __SwapElement(childA, childB) {
    const a = nodeIdOf(childA);
    const b = nodeIdOf(childB);
    if (a === b) {
      return undefined;
    }
    const attachedA = native.parentNode(a) !== null;
    const attachedB = native.parentNode(b) !== null;
    if (attachedA && attachedB) {
      native.swapElement(a, b);
    } else if (attachedA) {
      native.replaceElement(b, a);
    } else if (attachedB) {
      native.replaceElement(a, b);
    }
    return undefined;
  }

  /**
   * @param {unknown} newElement
   * @param {unknown} oldElement
   * @returns {undefined}
   */
  function __ReplaceElement(newElement, oldElement) {
    if (newElement === oldElement) {
      return undefined;
    }
    native.replaceElement(nodeIdOf(newElement), nodeIdOf(oldElement));
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
  function __SetClasses(element, classNames) {
    const nodeId = nodeIdOf(element);
    if (classNames) {
      native.setAttribute(nodeId, "class", String(classNames));
    } else {
      native.removeAttribute(nodeId, "class");
    }
    return undefined;
  }

  /**
   * @param {unknown} element
   * @param {unknown} id
   * @returns {undefined}
   */
  function __SetID(element, id) {
    const nodeId = nodeIdOf(element);
    if (id) {
      native.setAttribute(nodeId, "id", String(id));
    } else {
      native.removeAttribute(nodeId, "id");
    }
    return undefined;
  }

  /**
   * @param {unknown} element
   * @returns {string | null}
   */
  function __GetID(element) {
    return native.getAttribute(nodeIdOf(element), "id");
  }

  /**
   * The element's Lynx tag. web-core maps its HTML stand-in back
   * (`x-view` -> `view`); this runtime creates elements under the Lynx tag
   * itself, so the DOM's own local name is already the answer.
   *
   * @param {unknown} element
   * @returns {string}
   */
  function __GetTag(element) {
    return native.tagName(nodeIdOf(element));
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
  function __GetElementUniqueID(element) {
    if (!element) {
      return -1;
    }
    return nodeIdOf(element) ?? -1;
  }

  /**
   * A string is one complete style-attribute payload and is set verbatim. A
   * record is still a complete replacement, but the JavaScript side owns the
   * fan-out: it clears the old declaration block, hyphenates each key, and
   * sends one CSSOM-like `setProperty` operation per non-null value. Keeping
   * the fan-out here leaves the native boundary as one-property-only and
   * avoids inventing an object/array wire representation for `HostValue`.
   *
   * A falsy value removes the attribute. The `rpx`/`vw`/`vh`/`rem` token
   * rewriting web-core performs on the way through has no owner here yet, so
   * declarations reach stylo as authored.
   *
   * @param {unknown} element
   * @param {unknown} value
   * @returns {undefined}
   */
  function __SetInlineStyles(element, value) {
    const nodeId = nodeIdOf(element);
    if (!value) {
      native.removeAttribute(nodeId, "style");
      return undefined;
    }
    if (typeof value === "string") {
      native.setAttribute(nodeId, "style", value);
      return undefined;
    }
    // `__SetInlineStyles` replaces the whole inline declaration block. Start
    // from an explicitly empty attribute (rather than merely mutating the
    // properties mentioned by this record), then replay the new declarations
    // in object enumeration order so shorthand/longhand precedence is kept.
    // Keeping the empty attribute is observable for `{}` / all-nullish records
    // and matches web-core's complete-record setter.
    native.setAttribute(nodeId, "style", "");
    for (const [key, declaration] of Object.entries(value)) {
      if (declaration === null || declaration === undefined) {
        continue;
      }
      native.setNodeProperty(
        nodeId,
        hyphenate(key),
        String(declaration),
      );
    }
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
  function __SetCSSId(elements, cssId, entryName) {
    void elements;
    void cssId;
    void entryName;
    return undefined;
  }

  /**
   * `null`/`undefined` removes; anything else is stringified, which is what
   * web-core's `setElementPropertyOrAttribute` does for every name that is not
   * a live property of its HTML stand-in element. `id`, `class`, and `style`
   * reach their specialized DOM paths inside `bobcat.setAttribute`.
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
  function __SetAttribute(element, name, value) {
    if (name === "update-list-info") {
      throw new Error(
        "__SetAttribute(update-list-info) needs indexed child access, which the native boundary does not have",
      );
    }
    const nodeId = nodeIdOf(element);
    if (value === null || value === undefined) {
      native.removeAttribute(nodeId, String(name));
    } else {
      native.setAttribute(nodeId, String(name), String(value));
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
   * @returns {[Map<string, FiledHandler>, Map<string, FiledHandler>]}
   */
  function handlersFor(handle) {
    let maps = handlers.get(handle);
    if (maps === undefined) {
      maps = [new Map(), new Map()];
      handlers.set(handle, maps);
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
    let byName = indexed.get(handle);
    const state = byName?.get(name);
    if (wanted === (state?.[phase] ?? false)) {
      return undefined;
    }
    if (wanted) {
      native.enableEventListener(nodeIdOf(handle), phase, name);
    } else {
      native.disableEventListener(nodeIdOf(handle), phase, name);
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
      indexed.set(handle, byName);
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
    const lists = listeners.get(handle)?.get(name);
    const filed = handlers.get(handle)?.[STATIC].get(name);
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
    let byName = listeners.get(handle);
    if (byName === undefined) {
      byName = new Map();
      listeners.set(handle, byName);
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
    const byName = listeners.get(handle);
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
      handlers.get(handle)?.[slot].delete(name);
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
  function __AddEvent(element, eventType, eventName, handler) {
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
  function __GetEvent(element, eventName, eventType) {
    const type = String(eventType).toLowerCase();
    const name = String(eventName).toLowerCase();
    const slot = type === GLOBAL_BIND ? GLOBAL : STATIC;
    const filed = handlers
      .get(/** @type {object} */ (element))
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
  function __GetEvents(element) {
    const maps = handlers.get(/** @type {object} */ (element));
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
  function __SetEvents(element, events) {
    const handle = /** @type {object} */ (element);
    const maps = handlers.get(handle);
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
  function __UpdateListCallbacks(
    list,
    componentAtIndex,
    enqueueComponent,
    componentAtIndexes,
  ) {
    listCallbacks.set(/** @type {object} */ (list), {
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
   * @param {number} nodeId
   * @returns {object}
   */
  function targetInfo(nodeId) {
    return {
      id: native.getAttribute(nodeId, "id"),
      uid: nodeId,
      elementRefptr: handleOf(nodeId),
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
        native.stopPropagation();
      },
      stopImmediatePropagation: () => {
        entry.immediate = true;
        entry.stopped = true;
        native.stopPropagation();
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
   * Both stop methods reach `native.stopPropagation`, because the standard's
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
    const handle = handleOf(node);
    if (handle === undefined) {
      return dispatches.get(id);
    }
    const list = listeners.get(handle)?.get(name)?.[phase];
    const closures = list !== undefined && list.length > 0 ? list : undefined;
    const filed = handlers.get(handle)?.[STATIC].get(name);
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
  function __AddEventListener(element, eventName, callback, options) {
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
  function __RemoveEventListener(element, eventName, callback, options) {
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
  function __StopPropagation(event) {
    /** @type {{ stopPropagation?: () => void }} */ (event)?.stopPropagation?.();
    return undefined;
  }

  /**
   * @param {unknown} event
   * @returns {undefined}
   */
  function __StopImmediatePropagation(event) {
    /** @type {{ stopImmediatePropagation?: () => void }} */ (event)
      ?.stopImmediatePropagation?.();
    return undefined;
  }

  /** @returns {undefined} */
  function __FlushElementTree() {
    native.flushElementTree();
    return undefined;
  }

  // The host reads this back off the object it installed and calls it per
  // node per pass. Published rather than captured, which is the one direction
  // that has to travel this way: everything else on `bobcat` is native.
  bobcat.event_listener_callback = eventListenerCallback;

  Object.assign(globalThis, {
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
  });
})();
