// The `bobcat:transfer` ESM: HTML's StructuredSerializeWithTransfer as the
// engine's `postMessage`s run it — `Worker`'s (`bobcat-internal`), a worker
// global's (`bobcat:worker`) and the Context's (`bobcat:cross-thread-context`).
//
// The serializer and its transfer list are the host boundary's
// (`crates/quickjs-rust-bridge/AGENTS.md`, "A message can be written with a
// transfer list"): a host member registered with a transfer list writes the
// message with each listed object as a slot, and refuses an unlisted
// transferable, a listed value that is not one, and a duplicate, before it
// runs. Nothing here walks or rewrites a message. What is here: Web IDL's
// conversion of the list, the `DataCloneError` those refusals are, and the
// detaching of what was listed once the post succeeded.
//
// The transferable interfaces, `OffscreenCanvas` and `ImageBitmap`, are
// `bobcat:canvas`'s, which registers them here as it is evaluated. A realm
// that has not imported it has no transferable object: every non-empty list
// is refused by the serializer.

import { DOMException } from "bobcat:dom-exception";

/** What `bobcat:canvas` tells this module about its transferable objects. */
export interface Transferables {
  /**
   * The canvas-table id the host member is passed for one listed object: its
   * entry's id, 0 for an `OffscreenCanvas` whose context mode is not none,
   * which the host answers with HTML's `InvalidStateError`, and `undefined`
   * for an object that is not a live transferable, which the serializer
   * refuses before the host member runs.
   */
  id(value: object): number | undefined;
  /** Detaches one listed object once its entry has left the realm. */
  detach(value: object): void;
}

let transferables: Transferables | undefined;

/** Called once, by `bobcat:canvas`. */
export function registerTransferables(hooks: Transferables): void {
  transferables = hooks;
}

/** The serializer's refusals, which HTML calls a `DataCloneError`. */
const REFUSALS = new Set(["uncloneable object", "value is not transferable", "duplicate transferable"]);

/** `error` as `postMessage` throws it: one of the refusals above as a `DataCloneError`. */
export function cloneError(error: unknown): unknown {
  return error instanceof TypeError && REFUSALS.has(error.message)
    ? new DOMException(`The object could not be cloned: ${error.message}`, "DataCloneError")
    : error;
}

/** Web IDL's `sequence<object>` conversion. */
export function toObjectSequence(value: unknown): object[] {
  if ((typeof value !== "object" && typeof value !== "function") || value === null) {
    throw new TypeError("The transfer list is not a sequence");
  }
  const method = (value as { [Symbol.iterator]?: unknown })[Symbol.iterator];
  if (typeof method !== "function") throw new TypeError("The transfer list is not iterable");
  const list: object[] = [];
  const iterator = Reflect.apply(method, value, []) as Iterator<unknown>;
  for (let step = iterator.next(); !step.done; step = iterator.next()) {
    const item = step.value;
    if ((typeof item !== "object" && typeof item !== "function") || item === null) {
      throw new TypeError("A transfer list entry is not an object");
    }
    list.push(item);
  }
  return list;
}

/**
 * `postMessage`'s second argument, by Web IDL's overload resolution between
 * `(message, sequence<object> transfer)` and `(message, optional
 * StructuredSerializeOptions options = {})`: its list, or `undefined` for an
 * empty one, which posts as a message with no list does.
 */
export function transferOf(argument: unknown): object[] | undefined {
  if (argument === undefined || argument === null) return undefined;
  if (typeof argument !== "object" && typeof argument !== "function") {
    throw new TypeError("postMessage's second argument is neither a sequence nor a dictionary");
  }
  const list = (argument as { [Symbol.iterator]?: unknown })[Symbol.iterator] != null
    ? toObjectSequence(argument)
    : (argument as { transfer?: unknown }).transfer === undefined
      ? []
      : toObjectSequence((argument as { transfer?: unknown }).transfer);
  return list.length === 0 ? undefined : list;
}

/**
 * Posts one message whose list is `list` through `post`, which calls the host
 * member with the message, the list, and `ids` spread after it. Throws what
 * HTML throws: the serializer's refusals as a `DataCloneError`, and an
 * `OffscreenCanvas` with a context as an `InvalidStateError`, with nothing
 * posted and nothing detached. On success every listed object is detached.
 */
export function postTransferring(list: object[], post: (ids: number[]) => unknown): void {
  // An entry the serializer refuses never reaches the host member, so its
  // stand-in id is never read.
  const ids = list.map((item) => transferables?.id(item) ?? -1);
  let answer: unknown;
  try {
    answer = post(ids);
  } catch (error) {
    throw cloneError(error);
  }
  if (answer === "InvalidStateError") {
    throw new DOMException("An OffscreenCanvas with a rendering context cannot be transferred",
      "InvalidStateError");
  }
  for (const item of list) transferables!.detach(item);
}
