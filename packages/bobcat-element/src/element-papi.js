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
// | `__SwapElement(childA, childB)` | `bobcat.swapElement` |
// | `__FlushElementTree()` | `bobcat.flushElementTree` |
//
// Everything else — attributes, classes, inline styles, `__SetCSSId`, events,
// `__CreateFrame`, `__DropElement` (absent from every web-core generation),
// tree querying, and list callback execution — is not implemented. A bundle
// that reaches for another member fails at the missing global, not silently.
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
    parentNode: bobcat.parentNode,
    insertBefore: bobcat.insertBefore,
    removeElement: bobcat.removeElement,
    replaceElement: bobcat.replaceElement,
    swapElement: bobcat.swapElement,
    dropElement: bobcat.dropElement,
    flushElementTree: bobcat.flushElementTree,
  };

  const nodeIdSymbol = Symbol("nodeId");

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
   * @param {unknown} childA
   * @param {unknown} childB
   * @returns {undefined}
   */
  function __SwapElement(childA, childB) {
    native.swapElement(nodeIdOf(childA), nodeIdOf(childB));
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
    __FlushElementTree,
  });
})();
