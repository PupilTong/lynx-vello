/**
 * The native object the host installs on `globalThis` before evaluating
 * `element-papi.js`. It speaks DOM vocabulary over numeric `NodeId`s and
 * owns the document and the style/layout commit; misuse crashes at this
 * boundary instead of being validated.
 */
interface BobcatNative {
  /** Marks the permanent page live and returns its `NodeId`. */
  createPage(): number;
  /** Creates a detached element and returns its `NodeId`. */
  createElement(tag: string): number;
  setAttribute(nodeId: number, name: string, value: string): void;
  /** Reparenting insert; appends when `reference` is null. */
  insertBefore(parent: number, child: number, reference: number | null): void;
  /** Detaches `child` from its parent; a no-op when already detached. */
  removeElement(child: number): void;
  /** Replaces `oldElement` in place, leaving it detached but live. */
  replaceElement(newElement: number, oldElement: number): void;
  /** Frees one element, detaching its direct children. */
  dropElement(nodeId: number): void;
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
