/**
 * The native functions exported by `bobcat-internal:host`. They speak DOM
 * vocabulary over numeric `NodeId`s and own the document and style/layout
 * commit; misuse crashes at this boundary instead of being validated.
 */
interface BobcatNative {
  /**
   * Builds the realm's one document out of the ingredients the view staged
   * before the realm opened: its metrics, its fonts, its author sheets in
   * cascade order, and any image reports that arrived first. Throws when the
   * realm already has one.
   */
  createDocument(): void;
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
  setInlineStyleProperty(nodeId: number, name: string, value: string): void;
  supportsStyleProperty(name: string): boolean;
  queryElementIds(root: number, selector: string, firstOnly: 0 | 1): string;
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
  /**
   * The init data the view was given, as the string the host passed in —
   * unread and unparsed — or `undefined` when it was given none. Answers
   * once: the string is handed over, not kept.
   */
  initData(): string | undefined;
  /** The view's global props, handed over like `initData`. */
  globalProps(): string | undefined;
}

/**
 * The native functions a worker realm gets instead of the document. Its
 * `bobcat-internal:host` carries the timer pair and nothing else, because a
 * worker has no tree to mutate.
 */
interface BobcatWorkerNative {
  requestScript(id: string, path: string): void;
  readScript(path: string, timeout: number): string;
  /** Hands one JSON-encoded message back to the realm that created us. */
  postWorkerMessage(data: string): void;
  /**
   * Ends this worker once the running task returns. Queued messages and armed
   * timers go with it.
   */
  closeWorker(): void;
}

declare module "bobcat-internal:host" {
  export function notifyReady(): void;
  export function reportStartupFailure(message: string): void;
  export function runMtsJobs(continueAfterError?: boolean): boolean;
  export function evaluateScript(source: string, filename: string): unknown;
  export function reportScriptError(level: string, message: string): void;
  export function logScriptMessage(level: string, message: string): void;
  export function createWorker(url: string, name: string): string;
  export function sendWorkerMessage(key: string, data: string): void;
  export function terminateWorker(key: string): void;
  export const createDocument: BobcatNative["createDocument"];
  export const createPage: BobcatNative["createPage"];
  export const createElement: BobcatNative["createElement"];
  export const setAttribute: BobcatNative["setAttribute"];
  export const setInlineStyles: BobcatNative["setInlineStyles"];
  export const queryElementIds: BobcatNative["queryElementIds"];
  export const supportsStyleProperty: BobcatNative["supportsStyleProperty"];
  export const setInlineStyleProperty: BobcatNative["setInlineStyleProperty"];
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
  export const initData: BobcatNative["initData"];
  export const globalProps: BobcatNative["globalProps"];
}

declare module "bobcat-internal:worker" {
  export const requestScript: BobcatWorkerNative["requestScript"];
  export const readScript: BobcatWorkerNative["readScript"];
  export const postWorkerMessage: BobcatWorkerNative["postWorkerMessage"];
  export const closeWorker: BobcatWorkerNative["closeWorker"];
}
