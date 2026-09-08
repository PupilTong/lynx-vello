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
//
// `EventTarget` is imported rather than defined here: the worker realm needs
// the same class, on a runtime this module is not registered on, so the source
// is shared as a module and each runtime compiles its own copy.
//
// The one thing here that is not a sink is `Worker`. Lynx has no Worker on
// either target — the native engine registers no such global and web-core only
// uses the browser's own for its internal dual-thread plumbing — so this is a
// real W3C `Worker`, implemented as the standards policy asks rather than
// copied from a Lynx quirk that does not exist.

import { EventTarget, installEventHandler } from "bobcat:event-target";
import {
  createWorker,
  postWorkerMessage,
  terminateWorker,
} from "bobcat-internal:host";

function noop() {
  return undefined;
}

/**
 * Reads one object-shaped option without widening the public input.
 *
 * @param {unknown} options
 * @param {string} name
 * @returns {unknown}
 */
function objectOption(options, name) {
  return options && typeof options === "object"
    ? Reflect.get(options, name)
    : undefined;
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

// # `Worker`
//
// The realm keeps the `Worker` objects and their listeners; the host keeps the
// realm the script runs in, which is on another thread and another QuickJS
// runtime entirely. What crosses between them is a numeric key naming the
// worker and one JSON string per message.
//
// ## Deviations from HTML
//
// `postMessage` serializes with JSON rather than the structured clone
// algorithm: the two sides are separate heaps with a primitives-only boundary
// between them, so there is no object graph to clone. A cycle therefore throws
// a plain `TypeError` where the standard throws `DataCloneError`, `undefined`
// arrives as `null`, and `Map`/`Set`/`ArrayBuffer`/functions do not survive.
// There is no transfer list and no `MessagePort`, so a second argument is
// rejected rather than quietly ignored. `terminate()` is cooperative: it takes
// effect between the worker's tasks, because nothing interrupts a realm
// mid-call.

const workerKey = Symbol("workerKey");
const workerEnded = Symbol("workerEnded");

/**
 * Every live worker this realm created, by the key the host issued.
 *
 * A worker is held here for as long as it can still deliver, which is what
 * keeps one alive that a card started and dropped the last reference to —
 * the same reachability a browser gives a worker with pending activity.
 *
 * @type {Map<number, Worker>}
 */
const workers = new Map();

/**
 * Serializes one message for the host boundary.
 *
 * The array wrapper is what makes every payload a JSON document: bare
 * `undefined` has no JSON encoding, and a top-level scalar would otherwise
 * have to be special-cased on the way back.
 *
 * @param {unknown} data
 * @returns {string}
 */
function encodeMessage(data) {
  return JSON.stringify([data]);
}

/**
 * @param {string} data
 * @returns {unknown}
 */
function decodeMessage(data) {
  return JSON.parse(data)[0];
}

export class Worker extends EventTarget {
  /**
   * @param {unknown} scriptURL
   * @param {unknown} options
   */
  constructor(scriptURL, options) {
    super();
    if (scriptURL === undefined) {
      throw new TypeError("Worker requires a script URL");
    }
    const name = objectOption(options, "name");
    installEventHandler(this, "message");
    installEventHandler(this, "error");
    /** @type {boolean} */
    this[workerEnded] = false;
    /** @type {number} */
    this[workerKey] = createWorker(
      String(scriptURL),
      name === undefined ? "" : String(name),
    );
    workers.set(this[workerKey], this);
  }

  /**
   * @param {unknown} message
   * @param {unknown} transfer
   * @returns {undefined}
   */
  postMessage(message, transfer) {
    if (transfer !== undefined) {
      throw new TypeError("Bobcat's Worker.postMessage has no transfer list");
    }
    // A worker that has ended silently drops what is sent to it, which is
    // what a terminated worker does: `postMessage` is not where a card
    // learns the worker is gone.
    if (this[workerEnded]) {
      return undefined;
    }
    postWorkerMessage(this[workerKey], encodeMessage(message));
    return undefined;
  }

  /**
   * @returns {undefined}
   */
  terminate() {
    if (this[workerEnded]) {
      return undefined;
    }
    this[workerEnded] = true;
    workers.delete(this[workerKey]);
    terminateWorker(this[workerKey]);
    return undefined;
  }

  get [Symbol.toStringTag]() {
    return "Worker";
  }
}

/**
 * Delivers one thing a worker realm had to say. Called by the host, never by a
 * card.
 *
 * `kind` is one of four:
 *
 * - `"message"`, with the JSON payload the worker posted;
 * - `"error"`, with a failure message, from a worker that is still running —
 *   a timer callback that threw, which HTML reports without ending anything;
 * - `"failed"`, the same but from a worker whose realm is gone, which is what
 *   a script that threw on load or a realm that could not be built leaves;
 * - `"closed"`, when the worker ended itself with `close()`, which fires no
 *   event at all.
 *
 * A worker that failed or closed is forgotten here, because the host has
 * already dropped its realm and nothing sent afterwards could arrive.
 *
 * @param {number} key
 * @param {string} kind
 * @param {string} data
 * @returns {undefined}
 */
export function __BobcatDeliverWorkerEvent(key, kind, data) {
  const worker = workers.get(key);
  if (worker === undefined) {
    return undefined;
  }
  if (kind === "message") {
    worker.dispatchEvent({ type: "message", data: decodeMessage(data) });
    return undefined;
  }
  if (kind !== "error") {
    worker[workerEnded] = true;
    workers.delete(key);
  }
  if (kind !== "closed") {
    worker.dispatchEvent({ type: "error", message: data });
  }
  return undefined;
}

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
