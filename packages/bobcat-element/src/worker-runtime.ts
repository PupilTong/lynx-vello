import {
  type EventTarget,
  installEventHandler,
  installEventTarget,
} from "bobcat:event-target";
import { closeWorker, postWorkerMessage } from "bobcat-internal:worker";

// The `bobcat:worker` ESM: one worker realm's global scope, preloaded on the
// group's worker runtime and imported for its effect before the worker's own
// script.
//
// It plays the part `DedicatedWorkerGlobalScope` plays in HTML, and it is a
// module rather than a set of exports for the same reason `bobcat:timers` is:
// a worker script reaches `self`, `postMessage`, `close` and `onmessage` as
// free variables, the way it would in a browser.
//
// # What is here and what is not
//
// `postMessage`, `close`, `name`, `self`, the `message` event and the
// EventTarget surface under it. Not here: `importScripts` (this realm loads
// ESM, so a worker script uses `import`), `location`, `navigator`, `fetch`,
// `XMLHttpRequest`, `MessagePort`, `messageerror` (JSON cannot fail to
// deserialize what JSON produced), `onerror` (an uncaught exception in here is
// reported at the parent `Worker` and to the embedder, but this side has no
// hook to intercept it first), and the DOM — a worker realm holds no document
// and cannot reach one, which is the whole reason it is on another runtime and
// another thread.

/**
 * The worker realm's global scope once this module has run: an `EventTarget`
 * whose own members include `self`, `postMessage` and `close`.
 */
export interface WorkerGlobalScope extends EventTarget {
  self: WorkerGlobalScope;
  postMessage(message: unknown, transfer?: unknown): undefined;
  close(): undefined;
}

/**
 * The global scope object. `self` and `globalThis` are the same object, as
 * they are in a browser worker.
 */
const scope = globalThis as unknown as WorkerGlobalScope;

installEventTarget(scope);
installEventHandler(scope, "message");

// Bound copies of the three listener methods, as own properties of the
// global. A worker script writes `addEventListener("message", f)` with no
// receiver, and in a module that leaves `this` undefined — which the platform
// answers with WebIDL's rule that an operation on a [Global] interface
// defaults its receiver to the global. Binding is that rule, spelled once.
// The prototype chain `installEventTarget` built still stands underneath, so
// `globalThis instanceof EventTarget` remains true.
for (const method of [
  "addEventListener",
  "removeEventListener",
  "dispatchEvent",
] as const) {
  Object.defineProperty(scope, method, {
    configurable: true,
    writable: true,
    value: scope[method].bind(scope),
  });
}

function encodeMessage(data: unknown): string {
  return JSON.stringify([data]);
}

/**
 * Delivers one message the host took off this worker's queue.
 */
export function __BobcatDeliverWorkerMessage(data: string): undefined {
  scope.dispatchEvent({ type: "message", data: JSON.parse(data)[0] });
  return undefined;
}

Object.defineProperty(scope, "self", {
  configurable: true,
  enumerable: true,
  writable: true,
  value: scope,
});

Object.assign(scope, {
  postMessage(message: unknown, transfer?: unknown): undefined {
    if (transfer !== undefined) {
      throw new TypeError("Bobcat's postMessage has no transfer list");
    }
    postWorkerMessage(encodeMessage(message));
    return undefined;
  },
  /**
   * Ends this worker.
   *
   * Everything after this call in the running task still runs — the host
   * drops the realm only once that task returns — and nothing queued behind
   * it ever does: messages and armed timers are discarded from the moment
   * this is called, as HTML's closing flag requires.
   */
  close(): undefined {
    closeWorker();
    return undefined;
  },
});
