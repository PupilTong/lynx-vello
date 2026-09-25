import { EventTarget, installEventHandler } from "bobcat:event-target";
// The entry's own response URL, which `__BobcatInitEntry` names before the
// entry's body runs; a live binding, read at each construction.
// `bobcat:runtime` imports this module for its type alone, so the import
// below is no cycle.
import { __Card__ } from "bobcat:runtime";
import {
  createWorker,
  sendWorkerMessage,
  terminateWorker,
} from "bobcat-internal:host";

// Main-thread-only `bobcat-internal` exports. Each object owns a context on
// the group's existing worker thread. A message crosses as a structured clone:
// the host boundary serializes it with the engine's own serializer, so
// `undefined`, `NaN`, `Date`, `BigInt`, typed arrays, cycles and shared
// references survive, and a value it refuses — a function, a `Symbol`, a `Map`
// — throws synchronously here. Transfer lists remain unsupported.
const workers: Map<string, WeakRef<Worker>> = new Map();
const registry = new FinalizationRegistry<string>(key => {
  if (workers.delete(key)) terminateWorker(key);
});

/** A message the worker's script posted, as its `Worker` dispatches it. */
interface WorkerMessageEvent {
  type: "message";
  data: unknown;
  target: Worker;
}

/**
 * An error the host reported for the worker — an exception its script threw,
 * or its script or realm failing — with the fields of the host's report.
 */
interface WorkerErrorEvent {
  type: "error";
  message: string;
  filename: string;
  lineno: number;
  colno: number;
  target: Worker;
}

/** Main-thread-only Worker context constructor. */
export class Worker extends EventTarget {
  #key: string;
  declare onmessage: ((event: WorkerMessageEvent) => void) | null;
  declare onerror: ((event: WorkerErrorEvent) => void) | null;

  /**
   * Scripts run as modules, including when options are omitted. Only built-in
   * imports are available on the worker runtime today.
   */
  constructor(
    scriptURL: unknown,
    options: { name?: unknown; type?: string } | null = {},
  ) {
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
    // A relative URL resolves against the page's entry, as a browser's
    // resolves against the document that constructs the worker: the host
    // joins the two by URL rules and keeps no base URL of its own. One that
    // does not resolve is HTML's `SyntaxError`: the host answers `null` for
    // it rather than throwing, because every error a host member throws is
    // an `InternalError`, and the class is this realm's to choose.
    const key = createWorker(url, name, __Card__);
    if (key === null) {
      throw new SyntaxError(`Worker script URL \`${url}\` does not resolve against \`${__Card__}\``);
    }
    this.#key = key;
    this.onmessage = null;
    this.onerror = null;
    installEventHandler(this, "message");
    installEventHandler(this, "error");
    workers.set(this.#key, new WeakRef(this));
    registry.register(this, this.#key, this);
  }

  postMessage(message: unknown, transfer?: unknown) {
    if (transfer !== undefined) {
      throw new TypeError("Bobcat's postMessage has no transfer list");
    }
    // Serialized whether or not this worker still runs, as HTML's
    // StructuredSerialize comes before the "is it terminated" check: a
    // refused value throws here either way. The host drops a message for a
    // worker it no longer names.
    sendWorkerMessage(this.#key, message);
  }

  terminate() {
    registry.unregister(this);
    if (workers.delete(this.#key)) {
      terminateWorker(this.#key);
      this.dispatchEvent({ type: "__bobcat:close" });
    }
  }

  override get [Symbol.toStringTag]() {
    return "Worker";
  }
}

/**
 * A worker event already routed to its owning view. Ended workers discard
 * late events, including ones queued before terminate() on the other thread.
 */
export function __BobcatDispatchWorkerEvent(
  key: string,
  kind: string,
  data: unknown,
  filename: string,
  lineno: number,
  colno: number,
) {
  const worker = workers.get(key)?.deref();
  if (worker === undefined) return;
  if (kind === "closed" || kind === "failed") {
    workers.delete(key);
    registry.unregister(worker);
  }
  if (kind === "message") {
    worker.dispatchEvent({ type: "message", data, target: worker });
  } else if (kind === "error" || kind === "failed") {
    // A listener that throws is reported by the EventTarget walk and never
    // reaches here, so the close notification needs no guarding.
    worker.dispatchEvent({ type: "error", message: String(data), filename, lineno, colno, target: worker });
    if (kind === "failed") worker.dispatchEvent({ type: "__bobcat:close" });
  } else if (kind === "closed") {
    // Internal notification: MTS disposal must not await a reply from a
    // worker which has already closed or failed to open its realm.
    worker.dispatchEvent({ type: "__bobcat:close" });
  }
}
