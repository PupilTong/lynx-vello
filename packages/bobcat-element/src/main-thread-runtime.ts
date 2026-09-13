// The `bobcat:runtime` compatibility ESM imported by each transformed MTS entry.
//
// The JS Context and lifecycle/event calls reach this view's BTS Worker.
// Native modules remain sinks. Diagnostics reach the view's host; global
// events reach BTS through the same Worker FIFO as Context messages.
// The one local delivery path is `lynx.getEngine()`:
// its stable EventTarget retains realm-local listeners so `bobcat:boot` can
// dispatch `__RenderPage` when an entry has no legacy `globalThis.renderPage`.
// None of these bindings is installed on `globalThis`; the entry receives them
// through entry imports and, for native Scripts, global lexical bindings.
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
import { globalProps, initData, reportScriptError, logScriptMessage, notifyReady, reportStartupFailure, runMtsJobs, evaluateScript, loadStyleSheet, adoptStyleSheet, releaseStyleSheet } from "bobcat-internal:host";
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

// Native QuickContext::InternalCall drains jobs after a successful call and
// before its caller continues. A thrown call skips that nested drain; the
// enclosing runtime checkpoint still owns the jobs it left pending.
function callMts(method: Function, args: unknown[]) {
  let result;
  try { result = Reflect.apply(method, scope, args); }
  catch (error) { _ReportError(error); return undefined; }
  return runMtsJobs() ? result : undefined;
}

function dispatchEngineEvent(type: string, data: unknown) {
  dispatchEventListeners(engineContext, {type, data, origin:'Engine'},
    (callback, _receiver, event) => callMts(callback, [event]));
}

export { callMts as __BobcatCallMTS, dispatchEngineEvent as __BobcatDispatchEngineEvent };

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
      } else if (message.method === "reloadFromJS") {
        reloadPage(message.data, true);
        // Reload acknowledges the completed native lifecycle synchronously;
        // it does not share named RPC's await/callback continuation boundary.
        if (message.id !== undefined) {
          sendToBackground({bobcat: "runtime", method: "reloadResult", id: message.id});
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
let pageLoaded = false;
let initialProcessor = "";
let jsDataProcessor = false;

// Boot marks the initial MTS render, independently of the later BTS-ready
// promise. Native accepts reload once its template is loaded, even if BTS
// has not finished evaluating its app yet.
export function __BobcatPageLoaded() {
  pageLoaded = true;
}

export function __BobcatInitializeMTS(options: {
  initData?: unknown;
  globalProps?: Record<string, unknown>;
  globalPropsUpdates?: Record<string, unknown>;
  systemInfo?: Record<string, unknown>;
  processorName?: string;
  enableJSDataProcessor?: boolean;
}) {
  lynx.__initData = "initData" in options ? options.initData : __BobcatInitData;
  hostGlobalPropsJson = JSON.stringify({...("globalProps" in options ? options.globalProps : __globalProps), ...options.globalPropsUpdates});
  __globalProps = JSON.parse(hostGlobalPropsJson);
  lynx.__globalProps = __globalProps;
  SystemInfo = Object.freeze(options.systemInfo ?? {});
  lynx.SystemInfo = SystemInfo;
  updateScriptInputs?.(SystemInfo, __globalProps);
  initialProcessor = options.processorName ?? "";
  jsDataProcessor = options.enableJSDataProcessor === true;
}

export function __BobcatProcessInitData(data: unknown) { return __BobcatProcessData(data, initialProcessor); }

/** App::LoadApp separates encoded data from host TemplateData. React's
 * Fiber output does not set LepusInitData, so the encoded slot is native nil.
 */
export function __BobcatBackgroundData(data: unknown) {
  return {initData:null, updateData:data, processorName:jsDataProcessor ? initialProcessor : "", cacheData:[], globalProps:JSON.parse(hostGlobalPropsJson)};
}

export function __BobcatProcessData(data: unknown, processorName = "") {
  if (jsDataProcessor) return data;
  let candidate;
  try {
    const processData = scope["processData"];
    candidate = typeof processData === "function" ? callMts(processData, [data, processorName]) : data;
  }
  catch (error) { _ReportError(error); }
  // ProcessTemplateDataForFiber replaces data only for a table. Each native
  // context call reports an exception and returns to the assembler.
  return candidate !== null && typeof candidate === "object" && !Array.isArray(candidate) ? candidate : data;
}

export function __BobcatRenderPage(data: unknown) {
  const options = {preLoadTemplate:false, ...processorOptions(initialProcessor)};
  if (hasEventListener(engineContext, "__RenderPage")) dispatchEngineEvent('__RenderPage', [data, options]);
  else {
    const renderPage = scope["renderPage"];
    if (typeof renderPage === "function") callMts(renderPage, [data, options]);
  }
}

function processorOptions(name: string) { return jsDataProcessor ? {processorName:name} : {}; }

function updatePage(data: unknown, options: Record<string, unknown>) {
  try {
    if (hasEventListener(engineContext, "__UpdatePage")) dispatchEngineEvent('__UpdatePage', [data, options]);
    else {
      const update = scope["updatePage"];
      if (update != null) callMts(update as Function, [data, options]);
    }
  } catch (error) { _ReportError(error); }
}

function reloadPage(data: unknown, fromJS: boolean, processorName = "") {
  if (!pageLoaded && !fromJS) {
    _ReportError(new Error("ReloadTemplate in another loading process!!!"));
    return;
  }
  const processed = fromJS ? data : __BobcatProcessData(data, processorName);
  try {
    if (hasEventListener(engineContext, "__RemoveComponents")) dispatchEngineEvent('__RemoveComponents', []);
    else {
      const removeComponents = scope["removeComponents"];
      if (removeComponents != null) callMts(removeComponents as Function, []);
    }
  } catch (error) { _ReportError(error); }
  // Native enqueues OnJSAppReload before MTS updatePage can emit the next
  // first-screen event. React owns cleanup, data merging and rehydration.
  const name = jsDataProcessor && !fromJS ? processorName : "";
  sendToBackground({bobcat:"runtime", method:"onAppReload", args:[processed, {processorName:name}]});
  const options = {resetPageData:false, reloadFromJS:fromJS, reloadTemplate:true, nativeUpdateDataOrder:0, ...processorOptions(name)};
  updatePage(processed, options);
}

export function __BobcatApplyPageUpdate(json: string) {
  const message = JSON.parse(json);
  if (message.method === "updateGlobalProps") {
    updateGlobalProps(message.args[0]);
    return;
  }
  // Global events have already passed LynxView's readiness gate. Native's
  // data-update policy still ignores update/reset before the initial render.
  if (!pageLoaded && message.method === "updateCardData") return;
  const data = message.args[0];
  if (message.method === "onAppReload") {
    reloadPage(data, false, message.args[1].processorName);
    return;
  } else if (message.method === "updateCardData") {
    // The compiled MTS entry owns processData/updatePage and its data model.
    const name = message.args[1].processorName ?? "";
    const processed = __BobcatProcessData(data, name);
    updatePage(processed, {resetPageData:message.args[1].type === 1, reloadFromJS:false, reloadTemplate:false, nativeUpdateDataOrder:0, ...processorOptions(name)});
    // React's BTS registerDataProcessors is a no-op: its hook must receive
    // the same processed data as MTS, even if the MTS update reported an error.
    message.args[0] = processed;
    // Native's MTS path constructs fresh TemplateData with an empty name.
    message.args[1].processorName = jsDataProcessor ? name : "";
  }
  sendToBackground({ bobcat: "runtime", ...message });
}

function updateGlobalProps(data: Record<string, unknown>) {
  // Native's host merges literal top-level keys; TemplateAssembler receives
  // the complete props, not the diff. Notify BTS before entering MTS hooks.
  hostGlobalPropsJson = JSON.stringify({...JSON.parse(hostGlobalPropsJson), ...data});
  if (pageLoaded) sendToBackground({bobcat:"runtime", method:"updateGlobalProps", args:[JSON.parse(hostGlobalPropsJson)]});
  __globalProps = JSON.parse(hostGlobalPropsJson);
  lynx.__globalProps = __globalProps;
  updateScriptInputs?.(SystemInfo, __globalProps);
  if (!pageLoaded) return;
  try {
    if (hasEventListener(engineContext, "__UpdateGlobalProps")) dispatchEngineEvent('__UpdateGlobalProps', [__globalProps]);
    else {
      const update = scope["updateGlobalProps"];
      if (typeof update === "function") callMts(update, [__globalProps]);
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
type UpdateScriptInputs = (info: object, props: object) => void;
let updateScriptInputs: UpdateScriptInputs | undefined;

// Boot supplies the entry's named runtime/PAPI exports once. A native Script
// initializer retains these exact values in the global lexical environment;
// no global-object properties or Rust-owned JS value handles are involved.
export function __BobcatInstallScriptGlobals(bindings: Record<string, unknown>) {
  // Benchmark steps reuse the entry preamble on an already-booted realm.
  // Global lexical declarations can only be installed once.
  if (updateScriptInputs !== undefined) return;
  const names = Object.keys(bindings);
  if (names.some(name => !/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name))) {
    throw new TypeError("invalid Script binding name");
  }
  const initialize = evaluateScript(`let ${names.join(",")};
    values => {
      ({${names.join(",")}} = values);
      return (info, props) => { SystemInfo = info; __globalProps = props; };
    }`, "bobcat:script-bindings") as (values: Record<string, unknown>) => UpdateScriptInputs;
  updateScriptInputs = initialize(bindings);
}

export function __BobcatRegisterLepusChunks(chunks: Record<string, string>, evaluate: (source: string) => unknown) {
  lepusChunks = chunks;
  evaluateLepusChunk = evaluate;
}

export function __LoadLepusChunk(path: string, options: {dynamicComponentEntry?: string; chunkType?: number}) {
  if (arguments.length < 2 || typeof path !== "string" || options === null || typeof options !== "object") {
    throw new TypeError("__LoadLepusChunk requires a string path and options object");
  }
  const entry = options.dynamicComponentEntry;
  if (typeof entry === "string" && entry !== "__Card__") return false;
  if (!Object.hasOwn(lepusChunks, path) || !evaluateLepusChunk) return false;
  // Native TemplateEntry evaluates again on every call. Finding the chunk
  // returns true even when its evaluation reports a script exception.
  try { evaluateLepusChunk(lepusChunks[path]!); }
  catch (error) { _ReportError(error); }
  return true;
}

// Private execution policy; source-section lookup belongs to the bundle loader.
export function __BobcatEvaluateScript(source: string, filename: string) {
  // Native MTS returns the Script completion without Core's BTS init/cache.
  let result;
  try { result = evaluateScript(source, filename); }
  catch (error) {
    // QuickContext::EvalBuf logs a synchronous exception and leaves LoadScript's
    // default null result untouched. Jobs wait for the enclosing checkpoint.
    logScriptMessage("error", `QuickContext EvalBuf error: ${printable(error)}`);
    return null;
  }
  // Unlike InternalCall, EvalBuf retains its result after a failed job too.
  runMtsJobs(true);
  return result;
}

export const NativeModules = undefined;

const styleHandles = new WeakMap<object, string>();
const styleCleanup = new FinalizationRegistry<string>(id => {
  // A finalizer can run after native host members were revoked at teardown.
  try { releaseStyleSheet(id); } catch { /* realm teardown */ }
});

export function __LoadStyleSheet(key: string, bundleName: string): object | null {
  if (arguments.length < 2 || typeof key !== "string" || typeof bundleName !== "string") {
    throw new TypeError("__LoadStyleSheet requires a section key and bundle name");
  }
  const id = loadStyleSheet(key, bundleName);
  if (id === null) return null;
  const handle: object = Object.freeze(Object.create(null));
  styleHandles.set(handle, id);
  styleCleanup.register(handle, id);
  return handle;
}

export function __AdoptStyleSheet(handle: object) {
  const id = styleHandles.get(handle);
  if (id === undefined) throw new TypeError("__AdoptStyleSheet requires a stylesheet handle");
  adoptStyleSheet(id);
  return null;
}

export const lynx = {
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
