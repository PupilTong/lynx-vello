// The `bobcat:runtime` compatibility ESM imported by each transformed MTS entry.
//
// The JS Context and lifecycle/event calls reach this view's BTS Worker.
// Native modules and the error reporter remain sinks. A compiled chunk
// still probes those APIs while it installs
// the ReactLynx snapshot runtime, so this module exports explicit sinks for
// that bootstrap surface. The one local delivery path is `lynx.getEngine()`:
// its stable EventTarget retains realm-local listeners so `bobcat:boot` can
// dispatch `__RenderPage` when an entry has no legacy `globalThis.renderPage`.
// None of these bindings is installed on `globalThis`; the entry receives them
// only through the import declarations Bobcat prepends to its source.
//
// The host's page data arrives through `bobcat-internal:host` as the strings
// the view was given, and is parsed here as this module evaluates.
//
// This is not an Element PAPI implementation. Every `__*` element member,
// including the scoped-style sink `__SetCSSId`, belongs to element-papi.ts.
// Background-thread-only bindings such as `lynxCoreInject` also do not belong
// in this realm.
//
// `EventTarget` is imported rather than defined here: the worker realm needs
// the same class, on a runtime this module is not registered on, so the source
// is shared as a module and each runtime compiles its own copy.

import { EventTarget } from "bobcat:event-target";
import {
  type ContextEvent,
  createCrossThreadContext,
  packBtsMessage,
  unpackBtsMessage,
} from "bobcat:cross-thread-context";
import { globalProps, initData } from "bobcat-internal:host";
import type { Worker } from "bobcat-internal";

/**
 * What this realm sends its BTS Worker: a Context event, or a runtime call,
 * which its `bobcat` tag tells apart. A runtime call's `method` names what
 * the background runtime runs, and its other fields are that method's.
 */
type ToBackground =
  | ContextEvent
  | { bobcat: "runtime"; method: string; [field: string]: unknown };

/** The one runtime call the BTS Worker makes of this realm. */
type LepusMethodCall = {
  bobcat: "runtime";
  method: "callLepusMethod";
  name: string;
  data?: unknown;
  id?: number;
};

/**
 * What the BTS Worker sends this realm: that call, or a Context event's
 * public fields, which carry no `bobcat` tag.
 */
type FromBackground = LepusMethodCall | (ContextEvent & { bobcat?: never });

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
const jsContext = createCrossThreadContext();
const nativeContext = createContextSink();
const engineContext = new EventTarget();
// The realm's global object, where a card installs the methods
// `callLepusMethod` looks up by name.
const scope = globalThis as Record<string, unknown>;
let backgroundWorker: Worker | undefined;
let pendingBackgroundMessages: ReturnType<typeof packBtsMessage>[] = [];

function sendToBackground(message: ToBackground, isContext = false) {
  // A Context event can carry other user properties. Only its public fields
  // cross this channel, so an extra property cannot select our runtime calls.
  const wire = packBtsMessage(isContext
    ? { type: message.type, data: message.data }
    : message);
  if (backgroundWorker === undefined) pendingBackgroundMessages.push(wire);
  else backgroundWorker.postMessage(wire);
}

// Context events and runtime calls share the same FIFO, including calls the
// MTS entry makes before boot constructs its Worker. Snapshot at the call,
// including native values that plain JSON cannot represent.
jsContext.connect((event) => sendToBackground(event, true));

// Like web-worker-rpc, await the handler result before copying the reply.
// The ordinary realm checkpoint runs the continuation; no nested host entry.
async function callLepusMethod(message: LepusMethodCall) {
  try {
    const method = scope[message.name];
    const result = await (typeof method === "function"
      ? Reflect.apply(method, scope, [message.data]) : undefined);
    if (message.id !== undefined) {
      sendToBackground({bobcat: "runtime", method: "callLepusMethodResult", id: message.id, result});
    }
  } catch (error) {
    // A failed call or result encoding reports on the calling Worker, even
    // without a callback, through its existing unhandled-rejection path.
    sendToBackground({bobcat: "runtime", method: "callLepusMethodResult", id: message.id,
      error: {name: error instanceof Error ? error.name : "Error",
        message: error instanceof Error ? error.message : String(error)}});
  }
}

/**
 * Called by boot only after the MTS entry finishes importing. Entry-level
 * listeners already exist; events it sent before Worker construction are
 * flushed in order through the same Worker transport as later events.
 */
export function __BobcatConnectBackground(worker: Worker) {
  worker.addEventListener("message", (event: { data: unknown }) => {
    const message = unpackBtsMessage(event.data) as FromBackground;
    if (message?.bobcat === "runtime") {
      if (message.method === "callLepusMethod") {
        void callLepusMethod(message);
      }
    } else {
      jsContext.receive(message);
    }
  });
  backgroundWorker = worker;
  const queued = pendingBackgroundMessages;
  pendingBackgroundMessages = [];
  for (const message of queued) {
    worker.postMessage(message);
  }
}

export function __BobcatPublishEvent(
  componentId: unknown,
  handlerName: string,
  event: unknown,
) {
  sendToBackground({
    bobcat: "runtime",
    method: componentId ? "publicComponentEvent" : "publishEvent",
    args: componentId ? [componentId, handlerName, event] : [handlerName, event],
  });
}

engineContext.addEventListener("__DestroyLifetime", () => {
  sendToBackground({ bobcat: "runtime", method: "callDestroyLifetimeFun" });
});

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

/**
 * Parses one piece of the host's page data: the string the view was given,
 * which nothing native reads, or `undefined` when it was given none — `{}`
 * here, as in web-core. So this is where malformed JSON is first met, and the
 * error names which input it was: `JSON.parse`'s own error places the fault
 * in an anonymous `<input>`.
 */
function parsePageData(name: string, json: string | undefined): unknown {
  if (json === undefined) {
    return {};
  }
  try {
    return JSON.parse(json);
  } catch (error) {
    throw new SyntaxError(`${name} is not valid JSON: ${(error as Error).message}`);
  }
}

/** The host's init data, which boot hands to `processData`. */
export const __BobcatInitData = parsePageData("initData", initData());
export const __globalProps = parsePageData("globalProps", globalProps());

export function _AddEventListener() {
  return undefined;
}

export function _ReportError() {
  return undefined;
}

export function _SetSourceMapRelease() {
  return undefined;
}

export function __OnLifecycleEvent(data: unknown) {
  jsContext.dispatchEvent({ type: "__OnLifecycleEvent", data });
}

export const NativeModules = undefined;

export const lynx = {
  SystemInfo,
  __initData: {},
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
  getJSModule: function (name: unknown) {
    return name === "GlobalEventEmitter" ? globalEventEmitter : undefined;
  },
  registerDataProcessors: noop,
  reportError: _ReportError,
  triggerGlobalEventFromLepus: noop,
};
