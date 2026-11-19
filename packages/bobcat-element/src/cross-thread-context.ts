import { EventTarget } from "bobcat:event-target";
import { toObjectSequence } from "bobcat:transfer";

export type ContextEvent = { type: string; data?: unknown; origin?: string | undefined };
/** What a Context posts through: the envelope, and its transfer list if it has a non-empty one. */
export type ContextSender = (event: ContextEvent, transfer?: object[]) => void;
class CrossThreadContext extends EventTarget {
  #sender: ContextSender | undefined;
  #pending: [ContextEvent, object[] | undefined][] = [];
  #listeners = new WeakMap<Function, (event: unknown) => unknown>();
  #origin: string;
  constructor(origin: string) { super(); this.#origin = origin; }

  override addEventListener(type: unknown, listener: unknown, _options?: unknown) {
    if (typeof type !== "string" || typeof listener !== "function") {
      throw new TypeError("Context listener requires a string and function");
    }
    let wrapper = this.#listeners.get(listener);
    if (!wrapper) {
      wrapper = (event: unknown) => Reflect.apply(listener, undefined, [event]);
      this.#listeners.set(listener, wrapper);
    }
    // Native Context ignores DOM capture/once options and calls the closure
    // with undefined as receiver. EventTarget still owns listener walk/removal.
    return super.addEventListener(type, wrapper, undefined);
  }

  override removeEventListener(type: unknown, listener: unknown, _options?: unknown) {
    if (typeof type !== "string" || typeof listener !== "function") {
      throw new TypeError("Context listener requires a string and function");
    }
    return super.removeEventListener(type, this.#listeners.get(listener), undefined);
  }

  // @ts-expect-error Native Context returns the mediator's integer cancellation result.
  override dispatchEvent(event: ContextEvent) {
    if (event === null || typeof event !== "object" || typeof event.type !== "string" || !("data" in event)) {
      throw new TypeError("Context event requires a string type and a data property");
    }
    // Capture the public envelope now; payloads are copied by Worker's
    // structured-clone transport when posted, including messages queued
    // before connection.
    this.#send({ type: event.type, data: event.data, origin: this.#origin }, undefined);
    return 0;
  }

  /**
   * Sends a `message` event. `transfer` is an engine extension (Lynx's
   * Context has no such parameter; `docs/tracking/deviations.md`): HTML's
   * `postMessage` transfer list, which moves the `OffscreenCanvas` and
   * `ImageBitmap` objects it names to the other thread.
   */
  postMessage(message?: unknown, transfer?: unknown) {
    if (!arguments.length) throw new TypeError("Context postMessage requires an argument");
    const list = transfer === undefined ? undefined : toObjectSequence(transfer);
    this.#send({ type: "message", data: message, origin: this.#origin },
      list === undefined || list.length === 0 ? undefined : list);
  }

  #send(message: ContextEvent, transfer: object[] | undefined) {
    if (this.#sender === undefined) this.#pending.push([message, transfer]);
    else this.#sender(message, transfer);
  }

  connect(send: ContextSender) {
    this.#sender = send;
    const queued = this.#pending;
    this.#pending = [];
    for (const [event, transfer] of queued) send(event, transfer);
  }

  receive(event: ContextEvent) {
    super.dispatchEvent({ type: event.type, data: event.data, origin: event.origin });
  }
}

export function createCrossThreadContext(origin = "JSContext") {
  return new CrossThreadContext(origin);
}
