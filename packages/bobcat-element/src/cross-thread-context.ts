import { EventTarget } from "bobcat:event-target";

export type ContextEvent = { type: string; data?: unknown; origin?: string | undefined };
class CrossThreadContext extends EventTarget {
  #sender: ((event: ContextEvent) => void) | undefined;
  #pending: ContextEvent[] = [];
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
    // Capture the public envelope now; payloads are copied by Worker JSON
    // when posted, including messages queued before connection.
    const message = { type: event.type, data: event.data, origin: this.#origin };
    if (this.#sender === undefined) this.#pending.push(message);
    else this.#sender(message);
    return 0;
  }

  postMessage(message?: unknown) {
    if (!arguments.length) throw new TypeError("Context postMessage requires an argument");
    this.dispatchEvent({ type: "message", data: message });
  }

  connect(send: (event: ContextEvent) => void) {
    this.#sender = send;
    const queued = this.#pending;
    this.#pending = [];
    for (const event of queued) send(event);
  }

  receive(event: ContextEvent) {
    super.dispatchEvent({ type: event.type, data: event.data, origin: event.origin });
  }
}

export function createCrossThreadContext(origin = "JSContext") {
  return new CrossThreadContext(origin);
}
