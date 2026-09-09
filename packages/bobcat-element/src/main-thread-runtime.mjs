// @ts-check

// The `bobcat:runtime` compatibility ESM imported by each transformed MTS entry.
//
// The JS Context reaches this view's BTS Worker. Native modules, the error
// reporter and general lifecycle delivery remain sinks. A compiled chunk
// still probes those APIs while it installs
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

import { EventTarget } from "bobcat:event-target";
import { createCrossThreadContext } from "bobcat:cross-thread-context";

function noop() {
  return undefined;
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
const jsBridge = createCrossThreadContext();
const nativeContext = createContextSink();
const engineContext = new EventTarget();

/**
 * Called by boot only after the MTS entry finishes importing. Entry-level
 * listeners already exist; events it sent before Worker construction are
 * flushed in order through the same Worker transport as later events.
 * @param {import("./worker.mjs").Worker} worker
 */
export function __BobcatConnectBackground(worker) {
  worker.addEventListener("message", (/** @type {{ data: any }} */ event) => {
    jsBridge.receive(event.data);
  });
  jsBridge.connect((event) => worker.postMessage(event));
}

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
    return jsBridge.context;
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
