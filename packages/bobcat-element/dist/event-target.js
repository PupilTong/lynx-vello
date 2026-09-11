// Generated from src/event-target.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 d71565d6c08b36f4
// The realm-local `EventTarget` both preloaded runtimes build on, registered
// as the `bobcat:event-target` ESM.
//
// It is shared source rather than a shared object: the MTS realm and each
// worker realm live on different QuickJS runtimes, so no value could cross
// between them. What they share is this file, registered on both runtimes and
// compiled once per realm.
//
// Listeners never cross the host boundary. Registration identity and mutation
// during dispatch follow EventTarget's `(type, callback, capture)` rules; the
// capture bit participates in identity even on a standalone target with no
// ancestor path for it to reorder.
const eventTargetListeners = Symbol("eventTargetListeners");
/**
 * Reads one object-shaped listener option without widening the public input.
 */
function listenerOption(options, name) {
    return options && typeof options === "object"
        ? Reflect.get(options, name)
        : undefined;
}
function captureOf(options) {
    return typeof options === "boolean"
        ? options
        : Boolean(listenerOption(options, "capture"));
}
export class EventTarget {
    constructor() {
        this[eventTargetListeners] = new Map();
    }
    addEventListener(eventName, callback, options) {
        if (callback === null || callback === undefined) {
            return undefined;
        }
        if (typeof callback !== "function" && typeof callback !== "object") {
            throw new TypeError("an event listener must be a function or object");
        }
        const name = String(eventName);
        const capture = captureOf(options);
        let listeners = this[eventTargetListeners].get(name);
        if (listeners === undefined) {
            listeners = [];
            this[eventTargetListeners].set(name, listeners);
        }
        if (listeners.some((listener) => listener.callback === callback && listener.capture === capture)) {
            return undefined;
        }
        listeners.push({
            callback,
            capture,
            once: Boolean(listenerOption(options, "once")),
        });
        return undefined;
    }
    removeEventListener(eventName, callback, options) {
        if (callback === null || callback === undefined) {
            return undefined;
        }
        const name = String(eventName);
        const listeners = this[eventTargetListeners].get(name);
        if (listeners === undefined) {
            return undefined;
        }
        const capture = captureOf(options);
        const index = listeners.findIndex((listener) => listener.callback === callback && listener.capture === capture);
        if (index !== -1) {
            listeners.splice(index, 1);
            if (listeners.length === 0) {
                this[eventTargetListeners].delete(name);
            }
        }
        return undefined;
    }
    dispatchEvent(event) {
        if (event === null ||
            (typeof event !== "object" && typeof event !== "function")) {
            throw new TypeError("dispatchEvent requires an event object");
        }
        const name = String(Reflect.get(event, "type"));
        const listeners = this[eventTargetListeners].get(name);
        if (listeners === undefined) {
            return true;
        }
        // A snapshot prevents a listener added during this dispatch from running
        // in it. Looking each entry up in the live list also honors removals made
        // by an earlier callback.
        for (const listener of listeners.slice()) {
            const live = this[eventTargetListeners].get(name);
            if (live === undefined || !live.includes(listener)) {
                continue;
            }
            if (listener.once) {
                this.removeEventListener(name, listener.callback, listener.capture);
            }
            if (typeof listener.callback === "function") {
                listener.callback.call(this, event);
            }
            else {
                const handleEvent = Reflect.get(listener.callback, "handleEvent");
                if (typeof handleEvent === "function") {
                    handleEvent.call(listener.callback, event);
                }
            }
        }
        return Reflect.get(event, "defaultPrevented") !== true;
    }
    get [Symbol.toStringTag]() {
        return "EventTarget";
    }
}
/**
 * Makes an object that cannot extend the class one anyway — the worker
 * realm's global, which exists before any of this does.
 *
 * The prototype chain it builds is the standard's own
 * (`globalThis` → `EventTarget.prototype` → `Object.prototype`), so the
 * global answers `instanceof EventTarget` and carries the real methods rather
 * than three copies bolted onto it.
 */
export function installEventTarget(target) {
    Object.setPrototypeOf(target, EventTarget.prototype);
    Reflect.set(target, eventTargetListeners, new Map());
    return undefined;
}
/**
 * Installs one `on<name>` accessor whose value participates in dispatch as a
 * plain listener, the way an HTML event handler IDL attribute does.
 *
 * What is registered is an internal wrapper, never the assigned function
 * itself. That is what keeps `target.onmessage = f` and
 * `target.addEventListener("message", f)` two separate registrations: they
 * share no identity, so neither dedups against the other and removing one
 * leaves the other — which is what the DOM does, and what registering `f`
 * directly would get wrong in both directions.
 *
 * The wrapper is registered once, on the first assignment, and stays. So the
 * handler keeps its place in the listener order across reassignment, and
 * assigning `null` silences it without moving it — again as the DOM does.
 */
export function installEventHandler(target, name) {
    const handler = { current: null };
    function invoke(event) {
        if (typeof handler.current === "function") {
            handler.current.call(this, event);
        }
    }
    let registered = false;
    Object.defineProperty(target, `on${name}`, {
        configurable: true,
        enumerable: true,
        get() {
            return handler.current;
        },
        set(value) {
            handler.current = typeof value === "function" ? value : null;
            if (!registered) {
                this.addEventListener(name, invoke, false);
                registered = true;
            }
        },
    });
    return undefined;
}
