// @ts-check

import { EventTarget } from "bobcat:event-target";

/** @typedef {{ type: string, data?: unknown }} ContextEvent */

class CrossThreadContext extends EventTarget {
  /** @type {((event: ContextEvent) => void) | undefined} */
  #sender;
  /** @type {ContextEvent[]} */
  #pending = [];

  /** @param {ContextEvent} event */
  // @ts-expect-error Lynx ContextProxy dispatch returns 3, not EventTarget's boolean.
  dispatchEvent(event) {
    if (this.#sender === undefined) {
      // Keep the event itself until the Worker transport snapshots the send.
      this.#pending.push(event);
    } else {
      this.#sender(event);
    }
    return 3;
  }

  /** @param {unknown} [_message] */
  postMessage(_message) {
    // web-core's ContextProxy does not implement this separate operation.
    return undefined;
  }

  /** @param {(event: ContextEvent) => void} send */
  connect(send) {
    this.#sender = send;
    const queued = this.#pending;
    this.#pending = [];
    for (const event of queued) {
      send(event);
    }
  }

  /** @param {ContextEvent} event */
  receive(event) {
    // Local delivery calls the base method; this.dispatchEvent sends outward.
    super.dispatchEvent({ type: event.type, data: event.data ?? {} });
  }
}

export function createCrossThreadContext() {
  return new CrossThreadContext();
}
