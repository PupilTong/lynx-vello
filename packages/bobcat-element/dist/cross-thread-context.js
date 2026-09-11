// Generated from src/cross-thread-context.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 3ec6218a4701f1df
import { EventTarget } from "bobcat:event-target";
function encode(value, ancestors = new Set()) {
    if (value === undefined)
        return ["undefined"];
    if (typeof value === "number") {
        if (Object.is(value, -0))
            return ["number", "-0"];
        if (!Number.isFinite(value))
            return ["number", String(value)];
    }
    if (value === null || typeof value === "boolean" || typeof value === "number" || typeof value === "string") {
        return ["value", value];
    }
    if (typeof value !== "object")
        throw new TypeError(`cannot pass ${typeof value} across Lynx contexts`);
    if (ancestors.has(value))
        throw new TypeError("cannot pass a cyclic value across Lynx contexts");
    ancestors.add(value);
    try {
        if (Array.isArray(value))
            return ["array", Array.from(value, item => encode(item, ancestors))];
        return ["object", Object.keys(value).map(key => [key, encode(value[key], ancestors)])];
    }
    finally {
        ancestors.delete(value);
    }
}
function decode(value) {
    switch (value[0]) {
        case "undefined": return undefined;
        case "value": return value[1];
        case "number": return value[1] === "-0" ? -0 : Number(value[1]);
        case "array": return value[1].map(decode);
        case "object": return Object.fromEntries(value[1].map(pair => [pair[0], decode(pair[1])]));
        default: throw new TypeError("invalid Lynx value encoding");
    }
}
export function packBtsMessage(message) {
    return { bobcat: "value", value: encode(message) };
}
export function unpackBtsMessage(message) {
    const wire = message;
    return wire?.bobcat === "value" ? decode(wire.value) : message;
}
export function cloneBtsValue(value) { return decode(encode(value)); }
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
