// Generated from src/worker-runtime.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 9cf11acbfb9665eb
import { installEventHandler, installEventTarget, } from "bobcat:event-target";
import { closeWorker, postWorkerMessage } from "bobcat-internal:worker";
/**
 * The global scope object. `self` and `globalThis` are the same object, as
 * they are in a browser worker.
 */
const scope = globalThis;
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
]) {
    Object.defineProperty(scope, method, {
        configurable: true,
        writable: true,
        value: scope[method].bind(scope),
    });
}
function encodeMessage(data) {
    return JSON.stringify([data]);
}
/**
 * Delivers one message the host took off this worker's queue.
 */
export function __BobcatDeliverWorkerMessage(data) {
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
    postMessage(message, transfer) {
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
    close() {
        closeWorker();
        return undefined;
    },
});
