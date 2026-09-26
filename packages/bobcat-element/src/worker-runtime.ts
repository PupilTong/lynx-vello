import {
  type EventTarget,
  installEventHandler,
  installEventTarget,
  installExceptionReporter,
} from "bobcat:event-target";
import {
  closeWorker,
  postWorkerMessage,
  workerName,
} from "bobcat-internal:worker";
import { console } from "bobcat:diagnostics";

// The `bobcat:worker` ESM: one worker realm's global scope, preloaded on the
// group's worker runtime and imported for its effect by the worker's own
// script. The engine installs it in no realm: a worker's root module is the
// module at its URL, and nothing is evaluated before it. `bobcat:bts`, the
// BTS's root module, imports it as its first import, and a plain worker script
// that wants `postMessage` or `onmessage` writes `import "bobcat:worker";`. A
// realm in which this module never ran has no global scope, and the host drops
// a message posted to it: nothing there could receive one.
//
// It plays the part `DedicatedWorkerGlobalScope` plays in HTML, and it is a
// module rather than a set of exports for the same reason `bobcat:timers` is:
// a worker script reaches `self`, `postMessage`, `close` and `onmessage` as
// free variables, the way it would in a browser, once it has imported this
// module.
//
// # What is here and what is not
//
// `postMessage`, `close`, `name`, `self`, `reportError`, `console`, the
// `message` event and the EventTarget surface under it — including its rule
// that a listener which throws is reported here (through `reportError`) and
// the listeners behind it still run. `console` is `bobcat:diagnostics`'s, so
// what it prints reaches the embedder from this realm directly. `name` is
// read from the host in this module's last statement, which runs before any
// module the script imports after this one is evaluated, so such a module
// reads it at its top level too. The read is last because it is also how the
// host learns that this whole module has run, and so that the realm has a
// scope a posted message can be delivered to. Not here:
// `requestAnimationFrame` (a worker script imports it from
// `bobcat:animation-frame`, and `bobcat:bts-runtime` also gives it as an
// export and a `lynx` member; it is never a global), `importScripts` (this
// realm loads ESM, so a worker script uses `import`), `location`, `navigator`,
// `fetch`, `XMLHttpRequest`, `MessagePort`, `messageerror` (the reader cannot
// fail on what the same build's writer produced), `onerror` (an uncaught
// exception in here is reported at the parent `Worker` and to the embedder,
// but this side has no hook to intercept it first), and the DOM — a worker
// realm holds no document and cannot reach one, which is the whole reason it
// is on another runtime and another thread.

/**
 * The worker realm's global scope once this module has run: an `EventTarget`
 * whose own members include `self`, `name`, `console`, `postMessage` and
 * `close`.
 */
export interface WorkerGlobalScope extends EventTarget {
  self: WorkerGlobalScope;
  name: string;
  console: typeof console;
  postMessage(message: unknown, transfer?: unknown): undefined;
  close(): undefined;
  reportError(error: unknown): undefined;
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

/**
 * Delivers one message the host took off this worker's queue.
 */
export function __BobcatDeliverWorkerMessage(data: unknown): undefined {
  scope.dispatchEvent({ type: "message", data });
  return undefined;
}

Object.defineProperty(scope, "self", {
  configurable: true,
  enumerable: true,
  writable: true,
  value: scope,
});

// A namespace property, as WebIDL defines one for `console` on every global:
// writable and configurable, and not enumerable.
Object.defineProperty(scope, "console", {
  configurable: true,
  enumerable: false,
  writable: true,
  value: console,
});

Object.assign(scope, {
  postMessage(message: unknown, transfer?: unknown): undefined {
    if (transfer !== undefined) {
      throw new TypeError("Bobcat's postMessage has no transfer list");
    }
    postWorkerMessage(message);
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
  /**
   * Reports an error the way an uncaught exception in this realm is reported.
   */
  reportError(error: unknown): undefined {
    // This realm has no synchronous host hook. A rejection nothing handles is
    // reported by the checkpoint that ends the current entry, on the path an
    // uncaught exception takes: at the parent Worker's `error` event and to the
    // embedder as `WorkerThrew`, with this realm still running.
    void Promise.reject(error);
    return undefined;
  },
});

// Installed after `Object.assign` defines it: a listener exception in this
// realm is reported the way an uncaught one is, and the dispatch continues.
installExceptionReporter(scope.reportError);

// What the constructor named this worker, as a plain property of the global.
// The module's last statement: the host takes this read as the whole module
// having run, and delivers a posted message to this realm only after it.
scope.name = workerName();
