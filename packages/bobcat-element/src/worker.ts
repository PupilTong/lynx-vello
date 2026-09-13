import { EventTarget, installEventHandler } from "bobcat:event-target";
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
const workers: Map<string, Worker> = new Map();

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
    this.#key = createWorker(url, name);
    this.onmessage = null;
    this.onerror = null;
    installEventHandler(this, "message");
    installEventHandler(this, "error");
    workers.set(this.#key, this);
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
    if (workers.delete(this.#key)) {
      terminateWorker(this.#key);
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
  ...rest: unknown[]
) {
  const worker = workers.get(key);
  if (worker === undefined) return;
  if (kind === "closed" || kind === "failed") workers.delete(key);
  if (kind === "message") {
    worker.dispatchEvent({ type: "message", data: rest[0], target: worker });
  } else if (kind === "error" || kind === "failed") {
    worker.dispatchEvent({
      type: "error",
      message: String(rest[0]),
      filename: String(rest[1]),
      lineno: Number(rest[2]),
      colno: Number(rest[3]),
      target: worker,
    });
  }
}
