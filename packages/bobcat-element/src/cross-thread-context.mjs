// @ts-check

// Lynx's ContextProxy dispatch sends a typed event to the other realm. Its
// numeric result and unimplemented postMessage are distinct from EventTarget;
// only listener storage and local receive delivery use the shared EventTarget.
import { EventTarget, installEventTarget } from "bobcat:event-target";

/** @typedef {{ type: string, data?: unknown }} ContextEvent */

export function createCrossThreadContext() {
  /** @type {((event: ContextEvent) => void) | undefined} */
  let sender;
  /** @type {ContextEvent[]} */
  let pending = [];

  const context = {
    addEventListener: EventTarget.prototype.addEventListener,
    removeEventListener: EventTarget.prototype.removeEventListener,
    /** @param {ContextEvent} event */
    dispatchEvent(event) {
      if (sender === undefined) {
        // Keep the event itself, as web-core's pre-connection RPC queue does.
        // The Worker transport snapshots it only when it is actually sent.
        pending.push(event);
      } else {
        sender(event);
      }
      return 3;
    },
    /** @param {unknown} [_message] */
    postMessage(_message) {
      // web-core's ContextProxy does not implement this separate operation.
      return undefined;
    },
  };
  installEventTarget(context);

  return {
    context,
    /** @param {(event: ContextEvent) => void} send */
    connect(send) {
      sender = send;
      const queued = pending;
      pending = [];
      for (const event of queued) {
        send(event);
      }
    },
    /** @param {ContextEvent} event */
    receive(event) {
      // Invoke local dispatch directly: context.dispatchEvent sends outward.
      // installEventTarget initialized its listener state; Reflect.apply keeps
      // the public ContextProxy's numeric dispatch result out of that type.
      Reflect.apply(EventTarget.prototype.dispatchEvent, context, [
        { type: event.type, data: event.data ?? {} },
      ]);
    },
  };
}
