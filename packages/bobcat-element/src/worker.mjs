// @ts-check
import { EventTarget, installEventHandler } from "bobcat:event-target";
import {
  createWorker,
  sendWorkerMessage,
  terminateWorker,
} from "bobcat-internal:host";

// Main-thread-only `bobcat-internal` exports. Each object owns a context on
// the group's existing worker thread. Transport currently uses the worker
// scope's JSON encoding; structured clone and transfer lists are pending.
/** @type {Map<string, Worker>} */
const workers = new Map();

export class Worker extends EventTarget {
  #key;

  /**
   * Scripts run as modules, including when options are omitted. Only built-in
   * imports are available on the worker runtime today.
   * @param {unknown} scriptURL
   * @param {{ name?: unknown, type?: string } | null} [options]
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
    /** @type {((event: any) => void) | null} */
    this.onmessage = null;
    /** @type {((event: any) => void) | null} */
    this.onerror = null;
    installEventHandler(this, "message");
    installEventHandler(this, "error");
    workers.set(this.#key, this);
  }

  /** @param {unknown} message @param {unknown} [transfer] */
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
 * @param {string} key
 * @param {string} kind
 * @param {string} data
 */
export function __BobcatDispatchWorkerEvent(key, kind, data) {
  const worker = workers.get(key);
  if (worker === undefined) return;
  if (kind === "closed" || kind === "failed") workers.delete(key);
  if (kind === "message") {
    worker.dispatchEvent({
      type: "message", data: JSON.parse(data)[0], target: worker,
    });
  } else if (kind === "error" || kind === "failed") {
    worker.dispatchEvent({ type: "error", ...JSON.parse(data), target: worker });
  }
}
