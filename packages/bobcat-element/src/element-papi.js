// @ts-check
// The Lynx Element PAPI runtime.
//
// This file is the authoritative Element PAPI surface. It is evaluated as a
// classic script inside the QuickJS main-thread realm before any bundle code
// runs (bobcat-core embeds it with include_str!), and it is imported by the
// vitest suite for its side effects. It therefore uses no import/export
// syntax and reaches the outside world only through two globals:
//
// - `globalThis.bobcat` — the native object installed by the host before this
//   script runs. It speaks DOM vocabulary over numeric unique ids and owns
//   the document, structural validation, and the style/layout commit.
// - The `__*` Element PAPI globals this script assigns, mirroring web-core's
//   flat `Object.assign(mtsRealm.globalWindow, ...)` installation.
//
// # Element PAPI scope
//
// Every element constructor used by ReactLynx's Snapshot backend except
// `__CreateFrame`, plus the operations that make the resulting tree mutate,
// retire, and become visible:
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
// | `__DropElement(element)` | `bobcat.dropElement` |
// | `__FlushElementTree()` | `bobcat.flushElementTree` |
//
// Everything else — attributes, classes, inline styles, `__SetCSSId`, events,
// `__CreateFrame`, tree querying, and list callback execution — is not
// implemented. A bundle that reaches for another member fails at the missing
// global, not silently.
//
// # Identity and lifecycle
//
// - Element handles are plain objects created here, one per element. Every
//   PAPI return of an element yields the same object the element was created
//   with; a handle is unforgeable because identity lives in a private
//   WeakMap, not on the object.
// - Unique ids auto-increment from 1 (the permanent page) and are never
//   reused. This script owns the counter — seeded from the native table so a
//   fresh realm over a retained tree continues the sequence — and the native
//   side checks that ids arrive in sequence.
// - Dropping is the only retirement path. `__DropElement` retires exactly one
//   element immediately. As a garbage-collection backstop, every non-page
//   handle is registered with a FinalizationRegistry whose cleanup callback
//   queues the unique id; queued drops are applied by
//   `bobcat.deliverPendingElementDrops` (installed below), which the host
//   calls before each realm entry and after an explicit collection. Realm
//   teardown never delivers queued drops, so the last committed tree
//   survives the bootstrap realm.
// - `parentComponentUniqueID` is recorded, not honored: web-core uses it only
//   to inherit the parent component's CSS fragment id, and without
//   `__SetCSSId` there is nothing to inherit into. It is validated against
//   live elements and stored here. The page's `componentID` string is
//   validated and discarded.

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
    nextElementUniqueId: bobcat.nextElementUniqueId,
    setAttribute: bobcat.setAttribute,
    insertBefore: bobcat.insertBefore,
    removeElement: bobcat.removeElement,
    replaceElement: bobcat.replaceElement,
    dropElement: bobcat.dropElement,
    flushElementTree: bobcat.flushElementTree,
  };

  const PAGE_UNIQUE_ID = 1;
  const U32_MAX = 4294967295;
  const I32_MIN = -2147483648;
  const I32_MAX = 2147483647;

  // The monotonic unique-id allocator, seeded from the native table so a
  // fresh realm over a retained tree continues the sequence. Ids are never
  // reused, and the counter only advances on successful creation.
  let nextUniqueId = native.nextElementUniqueId();

  /**
   * Live elements by unique id. Presence means the element has not been
   * dropped; the page entry is permanent. The recorded fields are Lynx
   * bookkeeping the native side never reads.
   *
   * @type {Map<number, { parentComponentUniqueId: number, componentCssId: number }>}
   */
  const liveElements = new Map();
  liveElements.set(PAGE_UNIQUE_ID, {
    parentComponentUniqueId: 0,
    componentCssId: 0,
  });

  /**
   * The handle brand: an object is an element handle exactly when it has an
   * entry here.
   *
   * @type {WeakMap<object, number>}
   */
  const handleUniqueIds = new WeakMap();

  /**
   * Weak unique-id-to-handle index for future PAPI members that resolve ids
   * back to handles (`__GetElementByUniqueId` and friends).
   *
   * @type {Map<number, WeakRef<object>>}
   */
  const liveHandles = new Map();

  /**
   * Unique ids whose handles were collected; applied at the next delivery
   * point, never during collection itself.
   *
   * @type {number[]}
   */
  const pendingDrops = [];

  const registry = new FinalizationRegistry(
    (/** @type {number} */ uniqueId) => {
      pendingDrops.push(uniqueId);
    },
  );

  /** @type {object | null} */
  let pageHandle = null;
  let pageCreated = false;

  /**
   * @param {string} functionName
   * @param {unknown} value
   * @param {number} index
   * @returns {number}
   */
  function elementArgument(functionName, value, index) {
    const uniqueId =
      typeof value === "object" && value !== null
        ? handleUniqueIds.get(value)
        : undefined;
    if (uniqueId === undefined) {
      throw new Error(
        `${functionName} expects an element handle for argument ${index}`,
      );
    }
    return uniqueId;
  }

  /**
   * @param {string} functionName
   * @param {unknown} value
   * @param {number} index
   * @returns {number | null}
   */
  function optionalElementArgument(functionName, value, index) {
    if (value === undefined || value === null) {
      return null;
    }
    const uniqueId =
      typeof value === "object" ? handleUniqueIds.get(value) : undefined;
    if (uniqueId === undefined) {
      throw new Error(
        `${functionName} expects an element handle, null, or undefined for argument ${index}`,
      );
    }
    return uniqueId;
  }

  /**
   * @param {string} functionName
   * @param {unknown} value
   * @param {number} index
   * @returns {number}
   */
  function u32Argument(functionName, value, index) {
    if (typeof value !== "number") {
      throw new Error(
        `${functionName} expects a number for argument ${index}`,
      );
    }
    if (!Number.isInteger(value) || value < 0 || value > U32_MAX) {
      throw new Error(
        `${functionName} expects an unsigned 32-bit integer for argument ${index}, got ${value}`,
      );
    }
    return value;
  }

  /**
   * @param {string} functionName
   * @param {unknown} value
   * @param {number} index
   * @returns {number}
   */
  function i32Argument(functionName, value, index) {
    if (value === undefined || value === null) {
      return 0;
    }
    if (
      typeof value === "number" &&
      Number.isInteger(value) &&
      value >= I32_MIN &&
      value <= I32_MAX
    ) {
      return value;
    }
    throw new Error(`${functionName} expects an integer for argument ${index}`);
  }

  /**
   * @param {string} functionName
   * @param {unknown} value
   * @param {number} index
   * @returns {string}
   */
  function stringArgument(functionName, value, index) {
    if (typeof value === "string") {
      return value;
    }
    if (value === undefined || value === null) {
      return "";
    }
    throw new Error(`${functionName} expects a string for argument ${index}`);
  }

  /**
   * Creates the native element and records it live. The native side
   * validates the parent component (0 is the null sentinel; any other id
   * must be live) before the id sequence, exactly as before this runtime
   * existed, so a fresh realm over a retained tree still accepts elements
   * created by an earlier realm as parent components.
   *
   * @param {string} tag
   * @param {number} parentComponentUniqueId
   * @returns {number}
   */
  function allocateElement(tag, parentComponentUniqueId) {
    const uniqueId = nextUniqueId;
    native.createElement(tag, uniqueId, parentComponentUniqueId);
    nextUniqueId = uniqueId + 1;
    liveElements.set(uniqueId, {
      parentComponentUniqueId,
      componentCssId: 0,
    });
    return uniqueId;
  }

  /**
   * Mints the one handle object an element is ever identified by, and
   * registers its collection as a pending drop.
   *
   * @param {number} uniqueId
   * @returns {object}
   */
  function createHandle(uniqueId) {
    const handle = {};
    handleUniqueIds.set(handle, uniqueId);
    liveHandles.set(uniqueId, new WeakRef(handle));
    registry.register(handle, uniqueId, handle);
    return handle;
  }

  /**
   * @param {string} functionName
   * @param {string} tag
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function createParentedElement(functionName, tag, parentComponentUniqueID) {
    const parentComponentUniqueId = u32Argument(
      functionName,
      parentComponentUniqueID,
      0,
    );
    return createHandle(allocateElement(tag, parentComponentUniqueId));
  }

  /** @param {number} uniqueId */
  function retireElement(uniqueId) {
    liveElements.delete(uniqueId);
    liveHandles.delete(uniqueId);
  }

  /**
   * Applies queued garbage-collection drops. The host calls this before each
   * realm entry and after an explicit collection; a rogue early call only
   * applies drops that were already due.
   */
  function deliverPendingElementDrops() {
    while (pendingDrops.length > 0) {
      const uniqueId = pendingDrops.shift();
      if (uniqueId === undefined || !liveElements.has(uniqueId)) {
        // Retired through __DropElement after its handle was already
        // collected; nothing left to do.
        continue;
      }
      // The page handle is never registered, so the page is never queued.
      // Retirement is tolerant the way the collection backstop always was: a
      // bundle that retired this element through the bobcat object directly
      // must not fail an unrelated realm entry.
      try {
        native.dropElement(uniqueId);
      } catch {
        // The element was already gone natively.
      }
      retireElement(uniqueId);
    }
  }

  /**
   * @param {unknown} componentID
   * @param {unknown} componentCSSID
   * @returns {object}
   */
  function __CreatePage(componentID, componentCSSID) {
    stringArgument("__CreatePage", componentID, 0);
    const componentCssId = i32Argument("__CreatePage", componentCSSID, 1);
    native.createPage();
    if (!pageCreated) {
      pageCreated = true;
      const page = liveElements.get(PAGE_UNIQUE_ID);
      if (page !== undefined) {
        page.componentCssId = componentCssId;
      }
    }
    if (pageHandle === null) {
      // The page handle is permanent: repeated __CreatePage calls return the
      // same object, and the page is exempt from the collection backstop
      // because it can never be dropped.
      pageHandle = {};
      handleUniqueIds.set(pageHandle, PAGE_UNIQUE_ID);
      liveHandles.set(PAGE_UNIQUE_ID, new WeakRef(pageHandle));
    }
    return pageHandle;
  }

  /**
   * @param {unknown} tag
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateElement(tag, parentComponentUniqueID) {
    const tagName = stringArgument("__CreateElement", tag, 0);
    const parentComponentUniqueId = u32Argument(
      "__CreateElement",
      parentComponentUniqueID,
      1,
    );
    return createHandle(allocateElement(tagName, parentComponentUniqueId));
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateWrapperElement(parentComponentUniqueID) {
    return createParentedElement(
      "__CreateWrapperElement",
      "wrapper",
      parentComponentUniqueID,
    );
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateText(parentComponentUniqueID) {
    return createParentedElement(
      "__CreateText",
      "text",
      parentComponentUniqueID,
    );
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateImage(parentComponentUniqueID) {
    return createParentedElement(
      "__CreateImage",
      "image",
      parentComponentUniqueID,
    );
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateView(parentComponentUniqueID) {
    return createParentedElement(
      "__CreateView",
      "view",
      parentComponentUniqueID,
    );
  }

  /**
   * @param {unknown} parentComponentUniqueID
   * @returns {object}
   */
  function __CreateScrollView(parentComponentUniqueID) {
    return createParentedElement(
      "__CreateScrollView",
      "scroll-view",
      parentComponentUniqueID,
    );
  }

  /**
   * @param {unknown} text
   * @returns {object}
   */
  function __CreateRawText(text) {
    const literalText = stringArgument("__CreateRawText", text, 0);
    const uniqueId = allocateElement("raw-text", 0);
    native.setAttribute(uniqueId, "text", literalText);
    return createHandle(uniqueId);
  }

  /**
   * List construction records the element identity and tag only. The callback
   * arguments stay in JavaScript unretained until list callback execution
   * exists; declaring them keeps web-core's arity of 3.
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
    void componentAtIndex;
    void enqueueComponent;
    return createParentedElement(
      "__CreateList",
      "list",
      parentComponentUniqueID,
    );
  }

  /**
   * @param {unknown} parent
   * @param {unknown} child
   * @returns {object}
   */
  function __AppendElement(parent, child) {
    const parentUniqueId = elementArgument("__AppendElement", parent, 0);
    const childUniqueId = elementArgument("__AppendElement", child, 1);
    native.insertBefore(parentUniqueId, childUniqueId, null);
    return /** @type {object} */ (child);
  }

  /**
   * @param {unknown} parent
   * @param {unknown} child
   * @param {unknown} reference
   * @returns {object}
   */
  function __InsertElementBefore(parent, child, reference) {
    const parentUniqueId = elementArgument("__InsertElementBefore", parent, 0);
    const childUniqueId = elementArgument("__InsertElementBefore", child, 1);
    const referenceUniqueId = optionalElementArgument(
      "__InsertElementBefore",
      reference,
      2,
    );
    native.insertBefore(parentUniqueId, childUniqueId, referenceUniqueId);
    return /** @type {object} */ (child);
  }

  /**
   * @param {unknown} parent
   * @param {unknown} child
   * @returns {object}
   */
  function __RemoveElement(parent, child) {
    const parentUniqueId = elementArgument("__RemoveElement", parent, 0);
    const childUniqueId = elementArgument("__RemoveElement", child, 1);
    native.removeElement(parentUniqueId, childUniqueId);
    return /** @type {object} */ (child);
  }

  /**
   * @param {unknown} newElement
   * @param {unknown} oldElement
   * @returns {undefined}
   */
  function __ReplaceElement(newElement, oldElement) {
    const newUniqueId = elementArgument("__ReplaceElement", newElement, 0);
    const oldUniqueId = elementArgument("__ReplaceElement", oldElement, 1);
    native.replaceElement(newUniqueId, oldUniqueId);
    return undefined;
  }

  /**
   * @param {unknown} element
   * @returns {undefined}
   */
  function __DropElement(element) {
    const uniqueId = elementArgument("__DropElement", element, 0);
    if (!liveElements.has(uniqueId)) {
      // Dropping twice is tolerated: the second call is a no-op.
      return undefined;
    }
    // The page is permanently live, so this native call rejects it before any
    // bookkeeping changes.
    native.dropElement(uniqueId);
    retireElement(uniqueId);
    registry.unregister(/** @type {object} */ (element));
    return undefined;
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
    __DropElement,
    __FlushElementTree,
  });

  bobcat.deliverPendingElementDrops = deliverPendingElementDrops;
})();
