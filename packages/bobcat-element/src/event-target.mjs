// @ts-check

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
 * @typedef {object} RuntimeEventListener
 * @property {Function | object} callback
 * @property {boolean} capture
 * @property {boolean} once
 */

/**
 * Reads one object-shaped listener option without widening the public input.
 *
 * @param {unknown} options
 * @param {string} name
 * @returns {unknown}
 */
function listenerOption(options, name) {
  return options && typeof options === "object"
    ? Reflect.get(options, name)
    : undefined;
}

/**
 * @param {unknown} options
 * @returns {boolean}
 */
function captureOf(options) {
  return typeof options === "boolean"
    ? options
    : Boolean(listenerOption(options, "capture"));
}

export class EventTarget {
  constructor() {
    /** @type {Map<string, RuntimeEventListener[]>} */
    this[eventTargetListeners] = new Map();
  }

  /**
   * @param {unknown} eventName
   * @param {unknown} callback
   * @param {unknown} options
   * @returns {undefined}
   */
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
    if (
      listeners.some(
        (listener) =>
          listener.callback === callback && listener.capture === capture,
      )
    ) {
      return undefined;
    }
    listeners.push({
      callback,
      capture,
      once: Boolean(listenerOption(options, "once")),
    });
    return undefined;
  }

  /**
   * @param {unknown} eventName
   * @param {unknown} callback
   * @param {unknown} options
   * @returns {undefined}
   */
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
    const index = listeners.findIndex(
      (listener) =>
        listener.callback === callback && listener.capture === capture,
    );
    if (index !== -1) {
      listeners.splice(index, 1);
      if (listeners.length === 0) {
        this[eventTargetListeners].delete(name);
      }
    }
    return undefined;
  }

  /**
   * @param {unknown} event
   * @returns {boolean}
   */
  dispatchEvent(event) {
    if (
      event === null ||
      (typeof event !== "object" && typeof event !== "function")
    ) {
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
      } else {
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
 *
 * @param {object} target
 * @returns {undefined}
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
 * The handler is kept in the accessor's own closure and (de)registered on
 * assignment, so `target.onmessage = f` and `addEventListener("message", f)`
 * deliver in the order they were set, and assigning `null` removes it.
 *
 * @param {object} target
 * @param {string} name
 * @returns {undefined}
 */
export function installEventHandler(target, name) {
  /** @type {{ current: Function | null }} */
  const handler = { current: null };
  Object.defineProperty(target, `on${name}`, {
    configurable: true,
    enumerable: true,
    get() {
      return handler.current;
    },
    /** @param {unknown} value */
    set(value) {
      if (handler.current !== null) {
        this.removeEventListener(name, handler.current, false);
        handler.current = null;
      }
      if (typeof value === "function") {
        handler.current = /** @type {Function} */ (value);
        this.addEventListener(name, handler.current, false);
      }
    },
  });
  return undefined;
}
