import {
  type EventTarget,
  installEventHandler,
  installEventTarget,
  installExceptionReporter,
} from "bobcat:event-target";
import {
  closeWorker,
  invokeNativeModule,
  postWorkerMessage,
} from "bobcat-internal:worker";

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
// `postMessage`, `close`, `name`, `self`, `reportError`, the `message` event
// and the EventTarget surface under it — including its rule that a listener
// which throws is reported here (through `reportError`) and the listeners
// behind it still run. Not here: `importScripts` (this realm
// loads ESM, so a worker script uses `import`), `location`, `navigator`,
// `fetch`, `XMLHttpRequest`, `MessagePort`, `messageerror` (the reader cannot
// fail on what the same build's writer produced), `onerror` (an uncaught
// exception in here is reported at the parent `Worker` and to the embedder,
// but this side has no hook to intercept it first), and the DOM — a worker
// realm holds no document and cannot reach one, which is the whole reason it
// is on another runtime and another thread.

/**
 * The worker realm's global scope once this module has run: an `EventTarget`
 * whose own members include `self`, `postMessage` and `close`.
 */
export interface WorkerGlobalScope extends EventTarget {
  self: WorkerGlobalScope;
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

// # NativeModules transport
//
// The BTS runtime builds the `NativeModules` object; this is the wire under
// it. Both halves travel as text, because both are JavaScript's own: the
// arguments are `JSON.stringify`d here and parsed by the embedder's module if
// it cares, and an answer comes back as JSON text this realm parses — the
// engine's rule that a payload only JavaScript owns stays opaque to Rust
// rather than acquiring a Rust model on the way through.
//
// A function argument cannot be stringified, so it is replaced with `null` and
// its index sent alongside; the host mints one `ModuleCallback` per index. Each
// is single-shot, as native's is (Android's `CallbackImpl.mInvoked`, iOS's
// `wrapperWasCalled`): the slot is cleared before the function runs, so a
// second answer for it — or a release arriving after an invocation — finds
// nothing and does nothing.

/**
 * The function arguments of every call still outstanding, by call id, sparse
 * by argument index. A call with no function arguments has no entry at all,
 * and an entry is deleted once its last function has been taken.
 */
const moduleCalls = new Map<number, (Function | undefined)[]>();
let nextModuleCall = 1;

/**
 * `NativeModules.<module>.<method>(...args)`: hands the call to the host and
 * returns `undefined`, because a module answers through its callbacks and
 * never through this call's result.
 */
export function callNativeModule(
  module: string,
  method: string,
  args: unknown[],
): undefined {
  const functions: (Function | undefined)[] = [];
  const indices: number[] = [];
  for (let index = 0; index < args.length; index++) {
    if (typeof args[index] === "function") {
      functions[index] = args[index] as Function;
      indices.push(index);
    }
  }
  // Serialized before anything is registered: `JSON.stringify` throws on a
  // BigInt or a cycle, and the throw belongs to the caller — a call that
  // never reached the host must leave no functions behind for an answer that
  // can never come.
  const encoded = JSON.stringify(
    args.map(argument => typeof argument === "function" ? null : argument),
  );
  const call = nextModuleCall++;
  if (indices.length) moduleCalls.set(call, functions);
  invokeNativeModule(call, module, method, encoded, indices.join(","));
  return undefined;
}

/**
 * One native module's answer, delivered by the host on this realm's own turn.
 *
 * `argumentsJson` is the JSON array to spread; `undefined` releases the
 * function without calling it, which is what a module that dropped its
 * callback owes. Either way the slot is cleared first, so the single-shot rule
 * holds even if the function calls back into the same module.
 */
export function __BobcatNativeModuleCallback(
  call: number,
  index: number,
  argumentsJson: string | undefined,
): undefined {
  const functions = moduleCalls.get(call);
  const callback = functions?.[index];
  if (functions) {
    functions[index] = undefined;
    if (!functions.some(entry => entry !== undefined)) moduleCalls.delete(call);
  }
  if (callback === undefined || argumentsJson === undefined) return undefined;
  try {
    callback(...JSON.parse(argumentsJson) as unknown[]);
  } catch (error) {
    // A module callback is application code like a listener, so it is
    // reported here and the realm goes on.
    scope.reportError(error);
  }
  return undefined;
}
