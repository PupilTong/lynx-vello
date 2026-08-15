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
// | `__GetElementUniqueID(element)` | the handle's own node id |
// | `__SetInlineStyles(element, value)` | `bobcat.setAttribute` / `bobcat.removeAttribute` |
// | `__SetAttribute(element, name, value)` | `bobcat.setAttribute` / `bobcat.removeAttribute` |
// | `__AddEvent(element, eventType, eventName, handler)` | this runtime's own store |
// | `__GetEvent(element, eventName, eventType)` | this runtime's own store |
// | `__GetEvents(element)` | this runtime's own store |
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
// # What the recorded state does not yet reach
//
// `__AddEvent` records handlers and nothing dispatches to them: routing an
// input event to a handler (hit testing, Lynx's bind/catch phase walk, the
// gesture arena) belongs to a layer that does not exist yet. The registration
// a bundle makes is answered and kept; the subsystem behind it is absent
// rather than faked.
//
// # Identity and lifecycle
//
// - An element handle is a plain object carrying its DOM `NodeId` under a
//   realm-local symbol — web-core's `uniqueIdSymbol` shape; every PAPI
//   return of an element yields the same object it was created with.
//   `parentComponentUniqueID` and `__CreatePage`'s arguments are accepted
//   for PAPI shape and unused.
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
  };

  const nodeIdSymbol = Symbol("nodeId");

  const EVENT_KEY_SEPARATOR = ":";

  /**
   * The two handler slots web-core keeps per (event type, event name): a
   * background-thread handler name and a main-thread worklet.
   *
   * @typedef {{ crossThread: unknown, worklet: unknown }} EventSlots
   */

  /**
   * Registered event handlers per element handle. Weak by the handle, so a
   * registration can never keep its element alive: the entry dies with the
   * handle, which keeps collection the only release path even for a worklet
   * handler that refers back to the element it is bound to.
   *
   * @type {WeakMap<object, Map<string, EventSlots>>}
   */
  const eventHandlers = new WeakMap();

  /**
   * @param {unknown} eventType
   * @param {unknown} eventName
   * @returns {string}
   */
  function eventKey(eventType, eventName) {
    return `${String(eventType).toLowerCase()}${EVENT_KEY_SEPARATOR}${
      String(eventName).toLowerCase()
    }`;
  }

  const registry = new FinalizationRegistry(
    (/** @type {number} */ nodeId) => {
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
    registry.register(handle, nodeId, handle);
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
      // backstop: the page can never be dropped.
      pageHandle = { [nodeIdSymbol]: nodeId };
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
   * web-core's `hyphenate_style_name`: an uppercase letter becomes `-` plus
   * its lowercase form, so a React-shaped `backgroundColor` key reaches CSS as
   * `background-color`. ASCII only, because CSS property names are.
   *
   * @param {string} name
   * @returns {string}
   */
  function hyphenate(name) {
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
   * The one query web-core answers instead of crashing: a falsy or foreign
   * element reports `-1` rather than throwing, which is the contract its
   * callers read.
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
   * A string is set verbatim; a record is hyphenated and joined into one
   * declaration list, skipping null and undefined values, exactly as
   * web-core's `set_inline_styles_in_key_value_vec` does. A falsy value
   * removes the attribute. The `rpx`/`vw`/`vh`/`rem` token rewriting
   * web-core performs on the way through has no owner here yet, so
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
    let css = "";
    for (const [key, declaration] of Object.entries(value)) {
      if (declaration === null || declaration === undefined) {
        continue;
      }
      css += `${hyphenate(key)}:${String(declaration)};`;
    }
    native.setAttribute(nodeId, "style", css);
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
   * Registration only: a handler is recorded against its element, and nothing
   * dispatches to it — the routing half (hit testing, Lynx's bind/catch phase
   * walk, the gesture arena) belongs to a layer that does not exist yet.
   *
   * web-core's slot model, which this keeps: a string names a background-thread
   * handler, an object is a main-thread worklet, and the two occupy separate
   * slots for one (type, name) pair, so `bindtap` and `main-thread:bindtap` on
   * the same element do not evict each other. `null`/`undefined` clears both.
   * Both the type and the name are lowercased, as web-core lowercases them
   * before they reach its store.
   *
   * @param {unknown} element
   * @param {unknown} eventType
   * @param {unknown} eventName
   * @param {unknown} handler
   * @returns {undefined}
   */
  function __AddEvent(element, eventType, eventName, handler) {
    const key = eventKey(eventType, eventName);
    if (handler === null || handler === undefined) {
      const registrations = eventHandlers.get(/** @type {object} */ (element));
      if (registrations !== undefined) {
        registrations.delete(key);
      }
      return undefined;
    }
    /** @type {keyof EventSlots | undefined} */
    let slot;
    if (typeof handler === "string") {
      slot = "crossThread";
    } else if (typeof handler === "object") {
      slot = "worklet";
    }
    if (slot === undefined) {
      // web-core's chain covers strings, objects, and null; every other
      // handler shape falls through it unrecorded.
      return undefined;
    }
    let registrations = eventHandlers.get(/** @type {object} */ (element));
    if (registrations === undefined) {
      registrations = new Map();
      eventHandlers.set(/** @type {object} */ (element), registrations);
    }
    const slots = registrations.get(key) ??
      { crossThread: undefined, worklet: undefined };
    slots[slot] = handler;
    registrations.set(key, slots);
    return undefined;
  }

  /**
   * The background-thread handler alone, as web-core's `get_event` reads only
   * that slot. Note the argument order: name before type, the reverse of
   * `__AddEvent`.
   *
   * @param {unknown} element
   * @param {unknown} eventName
   * @param {unknown} eventType
   * @returns {unknown}
   */
  function __GetEvent(element, eventName, eventType) {
    const registrations = eventHandlers.get(/** @type {object} */ (element));
    return registrations?.get(eventKey(eventType, eventName))?.crossThread;
  }

  /**
   * Every recorded handler for one element, one entry per occupied slot, in
   * registration order.
   *
   * @param {unknown} element
   * @returns {{ type: string, name: string, function: unknown }[]}
   */
  function __GetEvents(element) {
    /** @type {{ type: string, name: string, function: unknown }[]} */
    const events = [];
    const registrations = eventHandlers.get(/** @type {object} */ (element));
    if (registrations === undefined) {
      return events;
    }
    for (const [key, slots] of registrations) {
      const separator = key.indexOf(EVENT_KEY_SEPARATOR);
      const type = key.slice(0, separator);
      const name = key.slice(separator + 1);
      if (slots.crossThread !== undefined) {
        events.push({ type, name, function: slots.crossThread });
      }
      if (slots.worklet !== undefined) {
        events.push({ type, name, function: slots.worklet });
      }
    }
    return events;
  }

  /** @returns {undefined} */
  function __FlushElementTree() {
    native.flushElementTree();
    return undefined;
  }

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
    __AddEvent,
    __GetEvent,
    __GetEvents,
    __FlushElementTree,
  });
})();
