// Generated from src/worker.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 a1a3a2879adb561c
import { EventTarget, installEventHandler } from "bobcat:event-target";
import { createWorker, sendWorkerMessage, terminateWorker, } from "bobcat-internal:host";
// Main-thread-only `bobcat-internal` exports. Each object owns a context on
// the group's existing worker thread. Transport currently uses the worker
// scope's JSON encoding; structured clone and transfer lists are pending.
const workers = new Map();
/** Main-thread-only Worker context constructor. */
export class Worker extends EventTarget {
    #key;
    /**
     * Scripts run as modules, including when options are omitted. Only built-in
     * imports are available on the worker runtime today.
     */
    constructor(scriptURL, options = {}) {
        super();
        if (arguments.length === 0) {
            throw new TypeError("Worker requires a script URL");
        }
        const url = String(scriptURL);
        if (options != null && typeof options !== "object") {
            throw new TypeError("Worker options must be an object");
        }
        if (options?.type !== undefined && options.type !== "module") {
            throw new TypeError("Bobcat workers support only module scripts");
        }
        const name = options?.name === undefined ? "" : String(options.name);
        this.#key = createWorker(url, name);
        this.onmessage = null;
        this.onerror = null;
        installEventHandler(this, "message");
        installEventHandler(this, "error");
        workers.set(this.#key, this);
    }
    postMessage(message, transfer) {
        if (transfer !== undefined) {
            throw new TypeError("Bobcat's postMessage has no transfer list");
        }
        const data = JSON.stringify([message]);
        if (workers.has(this.#key)) {
            sendWorkerMessage(this.#key, data);
        }
    }
    terminate() {
        if (workers.delete(this.#key)) {
            terminateWorker(this.#key);
        }
    }
    get [Symbol.toStringTag]() {
        return "Worker";
    }
}
/**
 * A worker event already routed to its owning view. Ended workers discard
 * late events, including ones queued before terminate() on the other thread.
 */
export function __BobcatDispatchWorkerEvent(key, kind, data) {
    const worker = workers.get(key);
    if (worker === undefined)
        return;
    if (kind === "closed" || kind === "failed")
        workers.delete(key);
    if (kind === "message") {
        worker.dispatchEvent({
            type: "message", data: JSON.parse(data)[0], target: worker,
        });
    }
    else if (kind === "error" || kind === "failed") {
        worker.dispatchEvent({ type: "error", ...JSON.parse(data), target: worker });
    }
}
