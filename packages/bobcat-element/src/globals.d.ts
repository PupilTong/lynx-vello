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
  removeAttribute(nodeId: number, name: string): void;
  /** The attribute's value, or null when the element does not carry it. */
  getAttribute(nodeId: number, name: string): string | null;
  /** The element's local name, verbatim as it was created. */
  tagName(nodeId: number): string;
  /** The parent's `NodeId`, or null for a detached element. */
  parentNode(nodeId: number): number | null;
  /** Reparenting insert; appends when `reference` is null. */
  insertBefore(parent: number, child: number, reference: number | null): void;
  /** Detaches `child` from its parent; a no-op when already detached. */
  removeElement(child: number): void;
  /** Replaces `oldElement` in place, leaving it detached but live. */
  replaceElement(newElement: number, oldElement: number): void;
  /** Exchanges two distinct attached elements, in or across parents. */
  swapElement(childA: number, childB: number): void;
  /** Frees one element, detaching its direct children. */
  dropElement(nodeId: number): void;
  /** Commits pending mutations through style and layout. */
  flushElementTree(): void;
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
declare var __ReplaceElements: (
  parent?: unknown,
  newChildren?: unknown,
  oldChildren?: unknown,
) => undefined;
declare var __SwapElement: (childA?: unknown, childB?: unknown) => undefined;
declare var __SetClasses: (
  element?: unknown,
  classNames?: unknown,
) => undefined;
declare var __SetID: (element?: unknown, id?: unknown) => undefined;
declare var __GetID: (element?: unknown) => string | null;
declare var __GetTag: (element?: unknown) => string;
declare var __GetElementUniqueID: (element?: unknown) => number;
declare var __SetInlineStyles: (
  element?: unknown,
  value?: unknown,
) => undefined;
declare var __SetAttribute: (
  element?: unknown,
  name?: unknown,
  value?: unknown,
) => undefined;
declare var __AddEvent: (
  element?: unknown,
  eventType?: unknown,
  eventName?: unknown,
  handler?: unknown,
) => undefined;
declare var __GetEvent: (
  element?: unknown,
  eventName?: unknown,
  eventType?: unknown,
) => unknown;
declare var __GetEvents: (
  element?: unknown,
) => { type: string; name: string; function: unknown }[];
declare var __FlushElementTree: () => undefined;
