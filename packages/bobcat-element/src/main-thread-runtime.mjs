// @ts-check

// The `bobcat:runtime` compatibility ESM imported by each transformed MTS entry.
//
// Bobcat does not have the background-thread realm, cross-context transport,
// native-module registry, error reporter, or general lifecycle delivery path
// yet. A compiled main-thread chunk still probes those APIs while it installs
// the ReactLynx snapshot runtime, so this module exports explicit sinks for
// that bootstrap surface. The one local delivery path is `lynx.getEngine()`:
// its stable EventTarget retains realm-local listeners so `bobcat:boot` can
// dispatch `__RenderPage` when an entry has no legacy `globalThis.renderPage`.
// None of these bindings is installed on `globalThis`; the entry receives them
// only through the import declarations Bobcat prepends to its source.
//
// This is not an Element PAPI implementation. Every `__*` element member,
// including the scoped-style sink `__SetCSSId`, belongs to element-papi.mjs.
// Background-thread-only bindings such as `lynxCoreInject` also do not belong
// in this realm.

function noop() {
  return undefined;
}

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
 * The capture bit participates in EventTarget listener identity even though a
 * standalone target has no ancestor path on which capture could change order.
 *
 * @param {unknown} options
 * @returns {boolean}
 */
function captureOf(options) {
  return typeof options === "boolean"
    ? options
    : Boolean(listenerOption(options, "capture"));
}

/**
 * The in-realm EventTarget used by `lynx.getEngine()`.
 *
 * It deliberately stays JavaScript-owned: callbacks never cross the host
 * boundary, and the preloaded runtime module's single evaluation gives the
 * entry and `bobcat:boot` the same target. Registration identity and mutation
 * during dispatch follow EventTarget's `(type, callback, capture)` rules.
 */
class EventTarget {
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

function createContextSink() {
  return {
    postMessage: noop,
    addEventListener: noop,
    removeEventListener: noop,
    dispatchEvent: function () {
      // Lynx's ContextProxy result for an event suppressed before delivery.
      return 3;
    },
  };
}

const coreContext = createContextSink();
const jsContext = createContextSink();
const nativeContext = createContextSink();
const engineContext = new EventTarget();

const globalEventEmitter = {
  addListener: noop,
  removeListener: noop,
  removeAllListeners: noop,
  emit: noop,
  trigger: noop,
  toggle: noop,
};

const runtimePerformance = {
  _generatePipelineOptions: noop,
  _onPipelineStart: noop,
  _bindPipelineIdWithTimingFlag: noop,
  _markTiming: noop,
  profileStart: noop,
  profileEnd: noop,
  profileMark: noop,
  profileFlowId: function () {
    return 0;
  },
  isProfileRecording: function () {
    return false;
  },
};

export const SystemInfo = Object.freeze({});
const initData = {};
export const __globalProps = {};

export function _AddEventListener() {
  return undefined;
}

export function _ReportError() {
  return undefined;
}

export function _SetSourceMapRelease() {
  return undefined;
}

export function __OnLifecycleEvent() {
  return undefined;
}

export const NativeModules = undefined;

export const lynx = {
  SystemInfo,
  __initData: initData,
  __globalProps,
  performance: runtimePerformance,
  getCoreContext: function () {
    return coreContext;
  },
  getJSContext: function () {
    return jsContext;
  },
  getNative: function () {
    return nativeContext;
  },
  getEngine: function () {
    return engineContext;
  },
  /**
   * @param {unknown} name
   */
  getJSModule: function (name) {
    return name === "GlobalEventEmitter" ? globalEventEmitter : undefined;
  },
  registerDataProcessors: noop,
  reportError: _ReportError,
  triggerGlobalEventFromLepus: noop,
};
