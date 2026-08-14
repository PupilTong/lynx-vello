/**
 * The native object the host installs on `globalThis` before evaluating
 * `element-papi.js`. It speaks DOM vocabulary over numeric unique ids: the
 * document, structural validation, and the style/layout commit live behind
 * it, while tag vocabulary, unique-id allocation, handle lifecycle, and the
 * `__*` PAPI surface live in the script.
 */
interface BobcatNative {
  /** Marks the permanent page element live and the batch uncommitted. */
  createPage(): void;
  /**
   * Creates a detached element carrying `uniqueId`. Ids must arrive in
   * ascending sequence and are never reused.
   */
  createElement(tag: string, uniqueId: number): void;
  setAttribute(uniqueId: number, name: string, value: string): void;
  /** Reparenting insert; appends when `reference` is null. */
  insertBefore(
    parent: number,
    child: number,
    reference: number | null,
  ): void;
  /** Detaches `child` from `parent` without retiring either element. */
  removeElement(parent: number, child: number): void;
  /** Replaces `oldElement` in place, leaving it detached but live. */
  replaceElement(newElement: number, oldElement: number): void;
  /** Retires one element permanently, detaching its direct children. */
  dropElement(uniqueId: number): void;
  /** Commits pending mutations through style and layout. */
  flushElementTree(): void;
  /** Installed by `element-papi.js`; applies queued collection drops. */
  deliverPendingElementDrops?: (() => void) | undefined;
}

declare var bobcat: BobcatNative;

// The Element PAPI globals `element-papi.js` assigns. Handles are opaque
// objects whose identity lives in the runtime's private WeakMap.
declare var __CreatePage: (
  componentID?: unknown,
  componentCSSID?: unknown,
) => object;
declare var __CreateElement: (
  tag?: unknown,
  parentComponentUniqueID?: unknown,
) => object;
declare var __CreateWrapperElement: (
  parentComponentUniqueID?: unknown,
) => object;
declare var __CreateText: (parentComponentUniqueID?: unknown) => object;
declare var __CreateImage: (parentComponentUniqueID?: unknown) => object;
declare var __CreateView: (parentComponentUniqueID?: unknown) => object;
declare var __CreateScrollView: (parentComponentUniqueID?: unknown) => object;
declare var __CreateRawText: (text?: unknown) => object;
declare var __CreateList: (
  parentComponentUniqueID?: unknown,
  componentAtIndex?: unknown,
  enqueueComponent?: unknown,
) => object;
declare var __AppendElement: (parent?: unknown, child?: unknown) => object;
declare var __InsertElementBefore: (
  parent?: unknown,
  child?: unknown,
  reference?: unknown,
) => object;
declare var __RemoveElement: (parent?: unknown, child?: unknown) => object;
declare var __ReplaceElement: (
  newElement?: unknown,
  oldElement?: unknown,
) => undefined;
declare var __DropElement: (element?: unknown) => undefined;
declare var __FlushElementTree: () => undefined;
