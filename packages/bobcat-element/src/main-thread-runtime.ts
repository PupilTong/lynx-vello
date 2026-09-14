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

import { EventTarget, hasEventListener, dispatchEventListeners } from "bobcat:event-target";
import {
  type ContextEvent,
  createCrossThreadContext,
} from "bobcat:cross-thread-context";
import { __BobcatQueryNodes } from "bobcat:element";
import type { NodeQueryRequest } from "bobcat:selector-query";
import "bobcat:timers";
import { requestScriptFrame } from "bobcat-internal:host";
import { initialProcessor as getInitialProcessor, globalProps, initData, reportScriptError, logScriptMessage, notifyReady, reportStartupFailure, preloadStyleSheet, adoptStyleSheet } from "bobcat-internal:host";
import type { Worker } from "bobcat-internal";
import type { TimerGlobals } from "bobcat:timers";

const timers = globalThis as unknown as TimerGlobals;

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
  | { bobcat: "runtime"; method: "disposed" }
  | { bobcat: "runtime"; method: "backgroundFailed"; message: string }
  | { bobcat: "runtime"; method: "reloadFromJS"; data?: unknown; id?: number }
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

// Engine dispatch owns its listener error policy. All callers use the same
// EventTarget API, with Promise jobs left to the existing outer checkpoint.
class EngineContext extends EventTarget {
  override dispatchEvent(event: unknown): boolean {
    return dispatchEventListeners(this, event, (callback, receiver, value) => {
      try { Reflect.apply(callback, receiver, [value]); }
      catch (error) { _ReportError(error); }
    });
  }
}

const coreContext = createContextSink();
const jsContext = createCrossThreadContext("CoreContext");
const nativeContext = createContextSink();
const engineContext = new EngineContext();
// The realm's global object, where a card installs the methods
// `callLepusMethod` looks up by name.
const scope = globalThis as Record<string, unknown>;

/** The entry response URL supplied by the view's resource loader. */
export let __Card__: string;

/** Boot supplies the entry URL before importing the application's module. */
export function __BobcatInitEntry(url: string): void {
  __Card__ = url;
}

function cardURL(bundleName: string): string {
  return bundleName === "__Card__" ? __Card__ : bundleName;
}

function styleSheetURL(key: string, bundleName: string): string {
  const base = cardURL(bundleName);
  const suffixAt = base.search(/[?#]/);
  const path = suffixAt < 0 ? base : base.slice(0, suffixAt);
  const suffix = suffixAt < 0 ? "" : base.slice(suffixAt);
  const name = encodeURIComponent(key).replace(/[!~'()]/g,
    character => `%${character.charCodeAt(0).toString(16).toUpperCase()}`);
  const section = key === "CSS" ? "" : `${name}/`;
  return `${path.replace(/\/$/, "")}/${section}index.css${suffix}`;
}
let backgroundWorker: Worker | undefined;
const animationCallbacks = new Map<number, (milliseconds: number) => void>();
let nextAnimationId = 1;
function updateFrameRequest() {
  requestScriptFrame(animationCallbacks.size > 0);
}
export function __BobcatBeginFrame(milliseconds: number) {
  const mainIds = Array.from(animationCallbacks.keys());
  for (const id of mainIds) {
    const callback = animationCallbacks.get(id);
    animationCallbacks.delete(id);
    if (callback) {
      try { callback(milliseconds); }
      catch (error) { _ReportError(error); }
    }
  }
  updateFrameRequest();
}
let backgroundDisposal: Promise<void> | undefined;
let acknowledgeDisposal: (() => void) | undefined;
let pendingBackgroundMessages: ToBackground[] = [];
function sendToBackground(message: ToBackground) {
  if (backgroundDisposal) return;
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
export function __BobcatConnectBackground(worker: Worker, data: unknown) {
  if (backgroundDisposal) {
    worker.terminate();
    return;
  }
  worker.addEventListener("__bobcat:close", () => {
    if (backgroundWorker === worker) backgroundWorker = undefined;
    acknowledgeDisposal?.();
  });
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
      } else if (message.method === "disposed") {
        acknowledgeDisposal?.();
      } else if (message.method === "backgroundFailed") {
        reportStartupFailure(message.message);
      } else if (message.method === "reloadFromJS") {
        reloadPage(message.data, true);
        // Acknowledge after jobs already queued by the lifecycle hooks.
        if (message.id !== undefined) {
          void Promise.resolve().then(() => {
            sendToBackground({bobcat: "runtime", method: "reloadResult", id: message.id});
          });
        }
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
  // Snapshot initial data before queued events or render can mutate it.
  worker.postMessage({bobcat: "runtime", method: "initialize", ...__BobcatBackgroundData(data), systemInfo: SystemInfo});
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

function disposeBackground(): Promise<void> {
  if (backgroundDisposal) return backgroundDisposal;
  const worker = backgroundWorker;
  pendingBackgroundMessages = [];
  animationCallbacks.clear();
  updateFrameRequest();
  backgroundDisposal = new Promise<void>(resolve => {
    if (worker === undefined) resolve();
    else acknowledgeDisposal = resolve;
  }).finally(() => {
    worker?.terminate();
    backgroundWorker = undefined;
    acknowledgeDisposal = undefined;
  });
  worker?.postMessage({ bobcat: "runtime", method: "dispose" });
  return backgroundDisposal;
}

engineContext.addEventListener("__DestroyLifetime", () => { void disposeBackground(); });

/** MTS keeps its Worker alive until BTS has acknowledged app cleanup. */
export function __BobcatDispose(): Promise<void> {
  engineContext.dispatchEvent({ type: "__DestroyLifetime" });
  return disposeBackground();
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

export let SystemInfo: Readonly<Record<string, unknown>> = Object.freeze({});

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
export let __globalProps = parsePageData("globalProps", globalProps()) as Record<string, unknown>;
// Host state is separate from the copies the two script realms may mutate.
let hostGlobalPropsJson = "{}";
const hostInitialProcessor = getInitialProcessor() ?? "";
let initialProcessor = hostInitialProcessor;
let jsDataProcessor = false;

export function __BobcatInitializeMTS(options: {
  initData?: unknown;
  globalProps?: Record<string, unknown>;
  systemInfo?: Record<string, unknown>;
  processorName?: string;
  enableJSDataProcessor?: boolean;
}) {
  lynx.__initData = "initData" in options ? options.initData : __BobcatInitData;
  hostGlobalPropsJson = JSON.stringify("globalProps" in options ? options.globalProps : __globalProps);
  __globalProps = JSON.parse(hostGlobalPropsJson);
  lynx.__globalProps = __globalProps;
  SystemInfo = Object.freeze({platform: "headless", runtimeType: "quickjs", lynxSdkVersion: "4.1.0", ...options.systemInfo});
  lynx.SystemInfo = SystemInfo;
  initialProcessor = options.processorName ?? hostInitialProcessor;
  jsDataProcessor = options.enableJSDataProcessor === true;
}

export function __BobcatProcessInitData(data: unknown) { return __BobcatProcessData(data, initialProcessor); }

/** App::LoadApp separates encoded data from host TemplateData. React's
 * Fiber output does not set LepusInitData, so the encoded slot is native nil.
 */
function __BobcatBackgroundData(data: unknown) {
  return {initData:null, updateData:data, processorName:jsDataProcessor ? initialProcessor : "", cacheData:[], globalProps:JSON.parse(hostGlobalPropsJson)};
}

export function __BobcatProcessData(data: unknown, processorName = "") {
  if (jsDataProcessor) return data;
  let candidate;
  try {
    const processData = scope["processData"];
    candidate = typeof processData === "function" ? Reflect.apply(processData, scope, [data, processorName]) : data;
  }
  catch (error) { _ReportError(error); }
  // ProcessTemplateDataForFiber replaces data only for a table. Each native
  // context call reports an exception and returns to the assembler.
  return candidate !== null && typeof candidate === "object" && !Array.isArray(candidate) ? candidate : data;
}

export function __BobcatRenderPage(data: unknown) {
  const options = {preLoadTemplate:false, ...processorOptions(initialProcessor)};
  try {
    if (hasEventListener(engineContext, "__RenderPage")) engineContext.dispatchEvent({type: '__RenderPage', data: [data, options]});
    else {
      const renderPage = scope["renderPage"];
      if (typeof renderPage === "function") Reflect.apply(renderPage, scope, [data, options]);
    }
  } catch (error) { _ReportError(error); }
}

function processorOptions(name: string) { return jsDataProcessor ? {processorName:name} : {}; }

function updatePage(data: unknown, options: Record<string, unknown>) {
  try {
    if (hasEventListener(engineContext, "__UpdatePage")) engineContext.dispatchEvent({type: '__UpdatePage', data: [data, options]});
    else {
      const update = scope["updatePage"];
      if (update != null) Reflect.apply(update as Function, scope, [data, options]);
    }
  } catch (error) { _ReportError(error); }
}

function reloadPage(data: unknown, fromJS: boolean, processorName = "") {
  const processed = fromJS ? data : __BobcatProcessData(data, processorName);
  try {
    if (hasEventListener(engineContext, "__RemoveComponents")) engineContext.dispatchEvent({type: '__RemoveComponents', data: []});
    else {
      const removeComponents = scope["removeComponents"];
      if (removeComponents != null) Reflect.apply(removeComponents as Function, scope, []);
    }
  } catch (error) { _ReportError(error); }
  // Native enqueues OnJSAppReload before MTS updatePage can emit the next
  // first-screen event. React owns cleanup, data merging and rehydration.
  const name = jsDataProcessor && !fromJS ? processorName : "";
  sendToBackground({bobcat:"runtime", method:"onAppReload", args:[processed, {processorName:name}]});
  const options = {resetPageData:false, reloadFromJS:fromJS, reloadTemplate:true, nativeUpdateDataOrder:0, ...processorOptions(name)};
  updatePage(processed, options);
}

export function __BobcatReload(json: string, processorName: string) {
  reloadPage(JSON.parse(json), false, processorName);
}

export function __BobcatUpdateData(json: string, processorName: string, reset: boolean) {
  // The compiled MTS entry owns processData/updatePage and its data model.
  const processed = __BobcatProcessData(JSON.parse(json), processorName);
  updatePage(processed, {resetPageData:reset, reloadFromJS:false, reloadTemplate:false, nativeUpdateDataOrder:0, ...processorOptions(processorName)});
  // React's BTS registerDataProcessors is a no-op: its hook must receive
  // the same processed data as MTS, even if the MTS update reported an error.
  const options = {type:reset ? 1 : 0, processorName:jsDataProcessor ? processorName : ""};
  sendToBackground({bobcat:"runtime", method:"updateCardData", args:[processed, options]});
}

export function __BobcatSendGlobalEvent(name: string, json: string) {
  sendToBackground({bobcat:"runtime", method:"sendGlobalEvent", name, args:JSON.parse(json)});
}

export function __BobcatUpdateGlobalProps(json: string) {
  const data = JSON.parse(json);
  // Native's host merges literal top-level keys; TemplateAssembler receives
  // the complete props, not the diff. Notify BTS before entering MTS hooks.
  hostGlobalPropsJson = JSON.stringify({...JSON.parse(hostGlobalPropsJson), ...data});
  sendToBackground({bobcat:"runtime", method:"updateGlobalProps", args:[JSON.parse(hostGlobalPropsJson)]});
  __globalProps = JSON.parse(hostGlobalPropsJson);
  lynx.__globalProps = __globalProps;
  try {
    if (hasEventListener(engineContext, "__UpdateGlobalProps")) engineContext.dispatchEvent({type: '__UpdateGlobalProps', data: [__globalProps]});
    else {
      const update = scope["updateGlobalProps"];
      if (typeof update === "function") Reflect.apply(update, scope, [__globalProps]);
    }
  } catch (error) { _ReportError(error); }
}

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

let lepusChunks: Record<string, string> = {};
let evaluateLepusChunk: ((source: string) => unknown) | undefined;

export function __BobcatRegisterLepusChunks(chunks: Record<string, string>, evaluate: (source: string) => unknown) {
  lepusChunks = chunks;
  evaluateLepusChunk = evaluate;
}

export function __LoadLepusChunk(path: string, options: {dynamicComponentEntry?: string; chunkType?: number}) {
  if (arguments.length < 2 || typeof path !== "string" || options === null || typeof options !== "object") {
    throw new TypeError("__LoadLepusChunk requires a string path and options object");
  }
  const entry = options.dynamicComponentEntry;
  if (typeof entry === "string" && cardURL(entry) !== __Card__) return false;
  if (!Object.hasOwn(lepusChunks, path) || !evaluateLepusChunk) return false;
  // Native TemplateEntry evaluates again on every call. Finding the chunk
  // returns true even when its evaluation reports a script exception.
  try { evaluateLepusChunk(lepusChunks[path]!); }
  catch (error) { _ReportError(error); }
  return true;
}

export const NativeModules = undefined;

const styleURLs = new WeakMap<object, string>();

export function __LoadStyleSheet(key: string, bundleName: string): object {
  if (arguments.length < 2 || typeof key !== "string" || typeof bundleName !== "string") {
    throw new TypeError("__LoadStyleSheet requires a section key and bundle name");
  }
  const url = styleSheetURL(key, bundleName);
  preloadStyleSheet(url);
  const handle: object = Object.freeze(Object.create(null));
  styleURLs.set(handle, url);
  return handle;
}

export function __AdoptStyleSheet(handle: object) {
  const url = styleURLs.get(handle);
  if (url === undefined) throw new TypeError("__AdoptStyleSheet requires a stylesheet handle");
  adoptStyleSheet(url);
  return null;
}

export const lynx = {
  setTimeout: timers.setTimeout,
  setInterval: timers.setInterval,
  clearTimeout: timers.clearTimeout,
  clearInterval: timers.clearInterval,
  requestAnimationFrame(callback: (milliseconds: number) => void) {
    if (typeof callback !== "function") throw new TypeError("requestAnimationFrame requires a function");
    const id = nextAnimationId++;
    animationCallbacks.set(id, callback);
    updateFrameRequest();
    return id;
  },
  cancelAnimationFrame(id: number) {
    animationCallbacks.delete(id);
    updateFrameRequest();
  },
  SystemInfo,
  __initData: {} as unknown,
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
