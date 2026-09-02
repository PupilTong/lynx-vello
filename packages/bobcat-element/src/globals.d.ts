/**
 * The native functions exported by `bobcat-internal:host`. They speak DOM
 * vocabulary over numeric `NodeId`s and own the document and style/layout
 * commit; misuse crashes at this boundary instead of being validated.
 */
interface BobcatNative {
  /** Marks the permanent page live and returns its `NodeId`. */
  createPage(): number;
  /** Creates a detached element and returns its `NodeId`. */
  createElement(tag: string): number;
  setAttribute(nodeId: number, name: string, value: string): void;
  /**
   * Replaces the whole inline declaration block from one record payload: a
   * flat sequence of `<utf16Length>:<text>` fields, name then value, two per
   * declaration. An empty payload leaves an empty `style` attribute.
   */
  setInlineStyles(nodeId: number, record: string): void;
  removeAttribute(nodeId: number, name: string): void;
  /** The attribute's value, or null when the element does not carry it. */
  getAttribute(nodeId: number, name: string): string | null;
  /** The element's local name, verbatim as it was created. */
  tagName(nodeId: number): string;
  /**
   * Every attribute name the element carries, in acquisition order, as one
   * record payload: a flat sequence of `<utf16Length>:<text>` fields, one per
   * name. Empty when the element carries none.
   */
  attributeNames(nodeId: number): string;
  /**
   * The `NodeId`s of the element's element children, in tree order, joined by
   * commas — no length prefix, because a decimal id cannot contain the
   * separator. Empty when the element has no element children. Child *nodes*
   * that are not elements, such as the text node a `raw-text` reflects, are
   * not in it.
   */
  childElementIds(nodeId: number): string;
  /** The parent's `NodeId`, or null for a detached element. */
  parentNode(nodeId: number): number | null;
  /** Reparenting insert; appends when `reference` is null. */
  insertBefore(parent: number, child: number, reference: number | null): void;
  /**
   * Detaches `child` from its parent; a no-op when already detached. Frees
   * nothing: the handle that names the child still holds it.
   */
  removeElement(child: number): void;
  /**
   * Replaces `oldElement` in place, leaving it detached — and live, held by
   * its handle.
   */
  replaceElement(newElement: number, oldElement: number): void;
  /** Exchanges two distinct attached elements, in or across parents. */
  swapElement(childA: number, childB: number): void;
  /**
   * Frees the element of a collected handle, and only it: its element
   * children are unlinked into detached roots, and what no handle could name
   * — the text node a `raw-text` reflects — goes with it.
   */
  dropElement(nodeId: number): void;
  /** Commits pending mutations through style and layout. */
  flushElementTree(): void;
  /**
   * Records that `nodeId` has at least one listener for `eventName` in the
   * given pass (`0` bubble, `1` capture), so the walk stops skipping it.
   */
  enableEventListener(nodeId: number, phase: number, eventName: string): void;
  /** The reverse: the last listener for that pair went away. */
  disableEventListener(nodeId: number, phase: number, eventName: string): void;
  /** Ends the walk in progress after the current node. */
  stopPropagation(): void;
  /**
   * Arms one timer `delayMilliseconds` from now — repeating until cleared
   * when `repeats` — and returns the id it is armed under. The delay goes
   * through HTML's `long` conversion and nesting clamp here, so any number
   * is accepted.
   */
  setTimer(delayMilliseconds: number, repeats: boolean): number;
  /** Disarms a timer, whether or not one is armed under that id. */
  clearTimer(id: number): void;
}

declare module "bobcat-internal:host" {
  export const createPage: BobcatNative["createPage"];
  export const createElement: BobcatNative["createElement"];
  export const setAttribute: BobcatNative["setAttribute"];
  export const setInlineStyles: BobcatNative["setInlineStyles"];
  export const removeAttribute: BobcatNative["removeAttribute"];
  export const getAttribute: BobcatNative["getAttribute"];
  export const tagName: BobcatNative["tagName"];
  export const attributeNames: BobcatNative["attributeNames"];
  export const childElementIds: BobcatNative["childElementIds"];
  export const parentNode: BobcatNative["parentNode"];
  export const insertBefore: BobcatNative["insertBefore"];
  export const removeElement: BobcatNative["removeElement"];
  export const replaceElement: BobcatNative["replaceElement"];
  export const swapElement: BobcatNative["swapElement"];
  export const dropElement: BobcatNative["dropElement"];
  export const flushElementTree: BobcatNative["flushElementTree"];
  export const enableEventListener: BobcatNative["enableEventListener"];
  export const disableEventListener: BobcatNative["disableEventListener"];
  export const stopPropagation: BobcatNative["stopPropagation"];
  export const setTimer: BobcatNative["setTimer"];
  export const clearTimer: BobcatNative["clearTimer"];
}

/**
 * Installed on this realm by the card's own bundled worklet runtime, and only
 * once a card actually compiles a main-thread function. `__AddEvent` reads it
 * per delivery rather than capturing it, because this file runs first.
 */
declare var runWorklet:
  | ((worklet: unknown, params: unknown[]) => void)
  | undefined;

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
  ...rest: unknown[]
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
declare var __GetChildren: (element?: unknown) => object[];
declare var __GetAttributeByName: (
  element?: unknown,
  name?: unknown,
) => string | null;
declare var __GetAttributeNames: (element?: unknown) => string[];
declare var __GetElementUniqueID: (element?: unknown) => number;
declare var __SetInlineStyles: (
  element?: unknown,
  value?: unknown,
) => undefined;
declare var __SetCSSId: (
  elements?: unknown,
  cssId?: unknown,
  entryName?: unknown,
) => undefined;
declare var __SetAttribute: (
  element?: unknown,
  name?: unknown,
  value?: unknown,
) => undefined;
declare var __UpdateListCallbacks: (
  list?: unknown,
  componentAtIndex?: unknown,
  enqueueComponent?: unknown,
  componentAtIndexes?: unknown,
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
declare var __SetEvents: (element?: unknown, events?: unknown) => undefined;
declare var __AddEventListener: (
  element?: unknown,
  eventName?: unknown,
  callback?: unknown,
  options?: unknown,
) => undefined;
declare var __RemoveEventListener: (
  element?: unknown,
  eventName?: unknown,
  callback?: unknown,
  options?: unknown,
) => undefined;
declare var __StopPropagation: (event?: unknown) => undefined;
declare var __StopImmediatePropagation: (event?: unknown) => undefined;
declare var __FlushElementTree: () => undefined;
