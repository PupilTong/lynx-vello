import { EventTarget } from "bobcat:event-target";

// Native ParseJSValue's data values, carried over the Worker's JSON channel.
// Tags are structural, so a user object can never collide with an undefined or
// special-number sentinel. Functions require native JSI object wrappers and
// cannot be smuggled across as empty objects.
type EncodedValue =
  | ["undefined"]
  | ["value", null | boolean | number | string]
  | ["number", string]
  | ["array", EncodedValue[]]
  | ["object", [string, EncodedValue][]];

function encode(value: unknown, ancestors = new Set<object>()): EncodedValue {
  if (value === undefined) return ["undefined"];
  if (typeof value === "number") {
    if (Object.is(value, -0)) return ["number", "-0"];
    if (!Number.isFinite(value)) return ["number", String(value)];
  }
  if (value === null || typeof value === "boolean" || typeof value === "number" || typeof value === "string") {
    return ["value", value];
  }
  if (typeof value !== "object") throw new TypeError(`cannot pass ${typeof value} across Lynx contexts`);
  if (ancestors.has(value)) throw new TypeError("cannot pass a cyclic value across Lynx contexts");
  ancestors.add(value);
  try {
    if (Array.isArray(value)) return ["array", Array.from(value, item => encode(item, ancestors))];
    return ["object", Object.keys(value).map(key => [key, encode((value as Record<string, unknown>)[key], ancestors)])];
  } finally { ancestors.delete(value); }
}

function decode(value: EncodedValue): unknown {
  switch (value[0]) {
    case "undefined": return undefined;
    case "value": return value[1];
    case "number": return value[1] === "-0" ? -0 : Number(value[1]);
    case "array": return value[1].map(decode);
    case "object": return Object.fromEntries(value[1].map(pair => [pair[0], decode(pair[1])]));
    default: throw new TypeError("invalid Lynx value encoding");
  }
}

export function packBtsMessage(message: unknown) {
  return { bobcat: "value" as const, value: encode(message) };
}

export function unpackBtsMessage(message: unknown): unknown {
  const wire = message as ReturnType<typeof packBtsMessage> | null | undefined;
  return wire?.bobcat === "value" ? decode(wire.value) : message;
}

export function cloneBtsValue(value: unknown) { return decode(encode(value)); }

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
