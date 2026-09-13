import { EventTarget } from "bobcat:event-target";

export type ContextEvent = { type: string; data?: unknown };

class CrossThreadContext extends EventTarget {
  #sender: ((event: ContextEvent) => void) | undefined;
  #pending: ContextEvent[] = [];

  // @ts-expect-error Lynx ContextProxy dispatch returns 3, not EventTarget's boolean.
  override dispatchEvent(event: ContextEvent) {
    if (this.#sender === undefined) {
      // Keep the event itself until the Worker transport snapshots the send.
      this.#pending.push(event);
    } else {
      this.#sender(event);
    }
    return 3;
  }

  postMessage(_message?: unknown) {
    // web-core's ContextProxy does not implement this separate operation.
    return undefined;
  }

  connect(send: (event: ContextEvent) => void) {
    this.#sender = send;
    const queued = this.#pending;
    this.#pending = [];
    for (const event of queued) {
      send(event);
    }
  }

  receive(event: ContextEvent) {
    // Local delivery calls the base method; this.dispatchEvent sends outward.
    super.dispatchEvent({ type: event.type, data: event.data ?? {} });
  }
}

export function createCrossThreadContext() {
  return new CrossThreadContext();
}
