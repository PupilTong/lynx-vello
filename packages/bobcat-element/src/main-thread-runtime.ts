// The `bobcat:runtime` compatibility ESM imported by each transformed MTS entry.
//
// The JS Context and lifecycle/event calls reach this view's BTS Worker.
// Native modules remain sinks. Diagnostics reach the view's host; global
// events reach BTS through the same Worker FIFO as Context messages.
// The one local delivery path is `lynx.getEngine()`:
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
} from "bobcat:cross-thread-context";
import { __BobcatQueryNodes } from "bobcat:element";
import type { NodeQueryRequest } from "bobcat:selector-query";
import { globalProps, initData, reportScriptError, logScriptMessage, notifyReady, reportStartupFailure } from "bobcat-internal:host";
import type { Worker } from "bobcat-internal";

/**
 * What this realm sends its BTS Worker: a Context event, or a runtime call,
 * which its `bobcat` tag tells apart. A runtime call's `method` names what
 * the background runtime runs, and its other fields are that method's.
 */
type ToBackground =
  | ContextEvent
  | { bobcat: "runtime"; method: string; [field: string]: unknown };

/** A named call the BTS Worker makes of this realm. */
type LepusMethodCall = {
  bobcat: "runtime";
  method: "callLepusMethod";
  name: string;
  data?: unknown;
  id?: number;
};

/**
 * What the BTS Worker sends this realm: a named call, a node query, or a
 * Context event's public fields, which carry no `bobcat` tag.
 */
type FromBackground = LepusMethodCall | NodeQueryRequest
  | { bobcat: "runtime"; method: "reportError" | "console"; level: string; message: string }
  | { bobcat: "runtime"; method: "backgroundReady" }
  | (ContextEvent & { bobcat?: never });

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
const jsContext = createCrossThreadContext("CoreContext");
const nativeContext = createContextSink();
const engineContext = new EventTarget();
// The realm's global object, where a card installs the methods
// `callLepusMethod` looks up by name.
const scope = globalThis as Record<string, unknown>;
let backgroundWorker: Worker | undefined;
let pendingBackgroundMessages: ToBackground[] = [];
function sendToBackground(message: ToBackground) {
  if (backgroundWorker === undefined) pendingBackgroundMessages.push(message);
  else backgroundWorker.postMessage(message);
}

// Context events and runtime calls share one FIFO before Worker connection.
// Only public Context fields cross it; extra event properties cannot select
// runtime methods. Payloads are copied by Worker's JSON transport when posted.
jsContext.connect((event) => sendToBackground({ type: event.type, data: event.data, origin: event.origin }));

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
    // A failed call or JSON serialization reports on the calling Worker, even
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
  worker.addEventListener("message", (event: { data: FromBackground }) => {
    const message = event.data;
    if (message?.bobcat === "runtime") {
      if (message.method === "nodeQuery") {
        try {
          const result = __BobcatQueryNodes(message);
          if (message.id !== undefined) sendToBackground({bobcat: "runtime", method: "nodeQueryResult", id: message.id, result});
        }
        catch (error) {
          _ReportError(error);
          const status = {code: 1, data: String(error)};
          const result = message.operation === "invoke" ? status : {status, data: message.token.first_only ? null : []};
          if (message.id !== undefined) sendToBackground({bobcat: "runtime", method: "nodeQueryResult", id: message.id, result});
        }
      } else if (message.method === "callLepusMethod") {
        void callLepusMethod(message);
      } else if (message.method === "backgroundReady") {
        notifyReady();
      } else if (message.method === "reportError") {
        reportScriptError(message.level, message.message);
      } else if (message.method === "console") {
        logScriptMessage(message.level, message.message);
      }
    } else {
      jsContext.receive(message);
    }
  });
  worker.addEventListener("error", (event: {message: string}) => {
    reportStartupFailure(event.message);
  });
  backgroundWorker = worker;
  const queued = pendingBackgroundMessages;
  pendingBackgroundMessages = [];
  for (const message of queued) {
    worker.postMessage(message);
  }
}

export function __BobcatApplyPageUpdate(json: string) {
  sendToBackground({ bobcat: "runtime", ...JSON.parse(json) });
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

export function _ReportError(error?: unknown, options?: {level?: string}) {
  const level = options?.level;
  reportScriptError(level === "warning" || level === "fatal" ? level : "error", printable(error));
}

function printable(value: unknown): string {
  if (value instanceof Error) {
    const summary = String(value);
    return value.stack?.includes(summary) ? value.stack
      : value.stack ? `${summary}\n${value.stack}` : summary;
  }
  if (typeof value === "string") return value;
  try { return JSON.stringify(value) ?? String(value); }
  catch { return String(value); }
}

export const console = Object.fromEntries(
  ["log", "info", "debug", "warn", "error"].map(level => [level,
    (...args: unknown[]) => logScriptMessage(level, args.map(printable).join(" ")),
  ]),
);


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
