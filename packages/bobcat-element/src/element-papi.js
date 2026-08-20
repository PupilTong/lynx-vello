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
// | `__CreateList(parentComponentUniqueID, ...)` | `bobcat.createElement` |
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
// | `__SetAttribute(element, name, value)` | `bobcat.setAttribute` / `bobcat.removeAttribute` |
// | `__AddEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__RemoveEventListener(element, name, callback, options?)` | this runtime's own store |
// | `__StopPropagation(event)` | `bobcat.stopPropagation` |
// | `__StopImmediatePropagation(event)` | `bobcat.stopPropagation` + this runtime's own store |
// | `__FlushElementTree()` | `bobcat.flushElementTree` |
//
// Everything else — `__CreateFrame`, `__DropElement` (absent from every
// web-core generation), `__SetCSSId`, `__AddClass`, `__AddInlineStyle`, the
// dataset, component-info, config, template-part and animation members, tree
// and selector querying, and list callback execution — is not implemented. A
// bundle that reaches for another member fails at the missing global, not
// silently.
//
// `__SetCSSId` is absent on purpose rather than by omission. Its whole job is
// to name the author-CSS scope an element cascades in, and no layer lowers a
// decoded `StyleInfo` into scoped author rules yet — so any encoding chosen
// here (web-core writes `l-css-id`/`l-e-name` attributes; native Lynx keeps
// css_id on the element) would be a design guess with nothing to validate it.
// It lands with the ingestion side that reads it.
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
// detail)` once per node per pass. Releasing first is what lets a callback
// mutate the tree. `phase` is the *pass* (`0` bubble, `1` capture), not the
// standard's `eventPhase`. They are different numbers, and at the target they
// do not even correspond, since both passes visit it; the event object's
// `eventPhase` is derived here, where the event object is.
//
// Propagation splits along the same line. Both stop methods call
// `bobcat.stopPropagation`, since the standard's `stopImmediatePropagation`
// implies `stopPropagation` and ending the walk is the host's. What stays
// here is the immediate half — skipping the rest of *this* node's listeners —
// because one delivery covers a whole node and the host has no finer step.
//
// # What is deliberately absent
//
// `__AddEvent`, `__GetEvent` and `__GetEvents` are gone. They stored a
// background-thread handler *name* and a worklet per (type, name) with
// overwrite semantics, and neither is deliverable here: cross-thread event
// delivery is out of scope, and a listener is now a plain callable. A bundle
// reaching for them fails at the missing global rather than registering into
// a store nothing reads.
//
// So are the pieces of `__AddEventListener` that depend on them: `closure_type`
// selecting a background handler string, and `bind_type` selecting Lynx's
// `catch` forms. A `catch` registration is a listener that calls
// `stopPropagation` first, which an author writes directly.
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
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateWrapperElement(parentComponentUniqueID) {
    void parentComponentUniqueID;
    return createHandle(native.createElement("wrapper"));
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateText(parentComponentUniqueID) {
    void parentComponentUniqueID;
    return createHandle(native.createElement("text"));
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateImage(parentComponentUniqueID) {
    void parentComponentUniqueID;
    return createHandle(native.createElement("image"));
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateView(parentComponentUniqueID) {
    void parentComponentUniqueID;
    return createHandle(native.createElement("view"));
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateScrollView(parentComponentUniqueID) {
    void parentComponentUniqueID;
    return createHandle(native.createElement("scroll-view"));
  }

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
   * List construction records the element identity and tag only; the
   * callbacks stay unretained until list callback execution exists.
   *
   * @param {unknown} parentComponentUniqueID
   * @param {unknown} componentAtIndex
   * @param {unknown} enqueueComponent
   * @returns {object}
   */
  function __CreateList(
    parentComponentUniqueID,
    componentAtIndex,
    enqueueComponent,
  ) {
    void parentComponentUniqueID;
    void componentAtIndex;
    void enqueueComponent;
    return createHandle(native.createElement("list"));
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
   * `null`/`undefined` removes; anything else is stringified, which is what
   * web-core's `setElementPropertyOrAttribute` does for every name that is not
   * a live property of its HTML stand-in element. `id`, `class`, and `style`
   * reach their specialized DOM paths inside `bobcat.setAttribute`.
   *
   * `update-list-info` is the one name that is not an attribute at all: it
   * drives list cell insertion and removal, and throws here rather than
   * writing a stringified command object onto the element.
   *
   * @param {unknown} element
   * @param {unknown} name
   * @param {unknown} value
   * @returns {undefined}
   */
  function __SetAttribute(element, name, value) {
    if (name === "update-list-info") {
      throw new Error(
        "__SetAttribute(update-list-info) needs the list surface, which is not implemented",
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
   * and no cross-boundary call happens at all.
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
    if (list.length === 1) {
      native.enableEventListener(nodeIdOf(handle), phase, name);
    }
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
    if (list.length === 0) {
      native.disableEventListener(nodeIdOf(handle), phase, name);
    }
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
   * @returns {undefined}
   */
  function eventListenerCallback(
    nodeId,
    targetNodeId,
    phaseId,
    eventName,
    detailJson,
  ) {
    const node = /** @type {number} */ (nodeId);
    const handle = handleOf(node);
    if (handle === undefined) {
      return undefined;
    }
    const name = String(eventName).toLowerCase();
    const phase = phaseId === CAPTURE ? CAPTURE : BUBBLE;
    const list = listeners.get(handle)?.get(name)?.[phase];
    if (list === undefined || list.length === 0) {
      return undefined;
    }

    let immediate = false;
    const event = {
      type: name,
      eventPhase: eventPhaseOf(node, targetNodeId, phase),
      target: targetInfo(/** @type {number} */ (targetNodeId)),
      currentTarget: targetInfo(node),
      detail: detailJson ? JSON.parse(String(detailJson)) : {},
      stopPropagation: () => {
        native.stopPropagation();
      },
      stopImmediatePropagation: () => {
        immediate = true;
        native.stopPropagation();
      },
    };

    // A copy, so a callback that adds or removes listeners for this same node
    // and name changes what the *next* event sees, not this one — the
    // standard's rule, one level down from the path the host froze.
    for (const registration of list.slice()) {
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
        return undefined;
      }
    }
    return undefined;
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
    __SetAttribute,
    __AddEventListener,
    __RemoveEventListener,
    __StopPropagation,
    __StopImmediatePropagation,
    __FlushElementTree,
  });
})();
