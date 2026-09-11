// Generated from src/cross-thread-context.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 49a18e1b89162300
import { EventTarget } from "bobcat:event-target";
class CrossThreadContext extends EventTarget {
    #sender;
    #pending = [];
    // @ts-expect-error Lynx ContextProxy dispatch returns 3, not EventTarget's boolean.
    dispatchEvent(event) {
        if (this.#sender === undefined) {
            // Keep the event itself until the Worker transport snapshots the send.
            this.#pending.push(event);
        }
        else {
            this.#sender(event);
        }
        return 3;
    }
    postMessage(_message) {
        // web-core's ContextProxy does not implement this separate operation.
        return undefined;
    }
    connect(send) {
        this.#sender = send;
        const queued = this.#pending;
        this.#pending = [];
        for (const event of queued) {
            send(event);
        }
    }
    receive(event) {
        // Local delivery calls the base method; this.dispatchEvent sends outward.
        super.dispatchEvent({ type: event.type, data: event.data ?? {} });
    }
}
export function createCrossThreadContext() {
    return new CrossThreadContext();
}
