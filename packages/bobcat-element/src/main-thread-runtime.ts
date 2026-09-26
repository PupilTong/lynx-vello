// The `bobcat:runtime` compatibility ESM imported by each transformed MTS entry.
//
// The JS Context and lifecycle/event calls reach this view's BTS Worker.
// This realm has no `NativeModules` of its own — Lepus has none — and only
// carries the embedder's module table to the BTS Worker, which does.
// Diagnostics reach the view's host; global
// events reach BTS through the same Worker FIFO as Context messages. This
// realm never waits on the BTS: a BTS that closed itself, failed, or trapped
// leaves the view running, and later messages go to its Worker all the same,
// where the host drops them as a browser drops a post to a terminated worker.
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
// the same class and cannot link this module, whose imports name
// `bobcat-internal:host` members only an MTS realm has. So the class is a
// module of its own: both runtimes register it, and each realm that imports it
// gets its own instance.

import { EventTarget, hasEventListener, installExceptionReporter } from "bobcat:event-target";
import {
  type ContextEvent,
  createCrossThreadContext,
} from "bobcat:cross-thread-context";
import { __BobcatQueryNodes } from "bobcat:element";
// The whole PAPI as one namespace, for the binding list a named Lepus
// chunk is called with: the export names are this module's only source of
// truth for what the entry preamble imports.
import * as elementPAPI from "bobcat:element";
import type { NodeQueryRequest } from "bobcat:selector-query";
import "bobcat:timers";
import { requestScriptFrame } from "bobcat-internal:host";
import { initialProcessor as getInitialProcessor, globalProps, initData, loadModuleSync, nativeModuleTable, reportScriptError, logScriptMessage, preloadStyleSheet, adoptStyleSheet } from "bobcat-internal:host";
import { sectionURL, styleSheetURL as sectionStyleSheetURL } from "bobcat:section-url";
import { type BundleHandle, createBundleFetches } from "bobcat:bundle-fetch";
import type { Worker } from "bobcat-internal";
import type { TimerGlobals } from "bobcat:timers";

const timers = globalThis as unknown as TimerGlobals;

// This realm reports a listener exception the way it reports any other script
// error: through the host's `reportScriptError`. Installed as this module
// evaluates, before anything here can dispatch.
installExceptionReporter(_ReportError);

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
  | { bobcat: "runtime"; method: "disposed" }
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
// A plain EventTarget: per-listener isolation is EventTarget's own, and the
// reporter installed above is this realm's. Promise jobs a listener schedules
// are left to the existing outer checkpoint.
const engineContext = new EventTarget();
// The realm's global object, where a card installs the methods
// `callLepusMethod` looks up by name.
const scope = globalThis as Record<string, unknown>;

/**
 * The entry's response URL, as the view's fetcher answered it. It names this
 * page's own container for chunks and stylesheets, and is the base a
 * `new Worker` specifier resolves against.
 */
export let __Card__: string;

/**
 * Names the entry: the host calls this with the entry's response URL before
 * completing the module boot imports it as, so `__Card__` is set before the
 * entry's body runs. Nothing calls it for a chunk.
 */
export function __BobcatInitEntry(url: string): void {
  __Card__ = url;
}

function cardURL(bundleName: string): string {
  return bundleName === "__Card__" ? __Card__ : bundleName;
}

/**
 * The resource URL one named stylesheet of that container lives at, which is
 * the string `named_style_url` writes in `crates/bobcat-source/src/page.rs`.
 * `__Card__` names this page's own container.
 */
function styleSheetURL(key: string, bundleName: string): string {
  return sectionStyleSheetURL(key, cardURL(bundleName));
}

/**
 * The resource URL one named Lepus chunk or custom section of that container
 * lives at, which is the string `named_chunk_url` writes in
 * `crates/bobcat-source/src/page.rs`.
 */
function chunkURL(name: string, bundleName: string): string {
  return sectionURL(name, cardURL(bundleName));
}

// Set once, at connection, and never cleared: a Worker that has ended is still
// the Worker this view posts to, and the host is what drops those posts.
let backgroundWorker: Worker | undefined;
// The BTS Worker ended — it closed itself, its script failed, or its thread
// trapped. Only disposal reads it: nothing can reply from an ended Worker.
let backgroundEnded = false;
const animationCallbacks = new Map<number, (milliseconds: number) => void>();
let nextAnimationId = 1;
let frameRequested = false;
function updateFrameRequest() {
  const pending = animationCallbacks.size > 0;
  if (pending === frameRequested) return;
  frameRequested = pending;
  requestScriptFrame(pending);
}
export function __BobcatBeginFrame(milliseconds: number) {
  frameRequested = false;
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
/**
 * The FIFO exists only for the messages a booting entry sends before its
 * Worker is constructed. Once connected, every message goes to the Worker,
 * whether or not the BTS still runs: the host drops a post to a worker it no
 * longer names, exactly as a browser drops a post to a terminated worker.
 * Queueing them instead would grow a list nothing can ever drain.
 */
function sendToBackground(message: ToBackground) {
  if (backgroundDisposal) return;
  if (backgroundWorker === undefined) pendingBackgroundMessages.push(message);
  else backgroundWorker.postMessage(message);
}

// Context events and runtime calls share one FIFO before Worker connection.
// Only public Context fields cross it; extra event properties cannot select
// runtime methods. Payloads are copied by Worker's structured-clone transport
// when posted.
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
    // A failed call, or a result value the transport refuses, reports on the
    // calling Worker — even without a callback — through its existing
    // unhandled-rejection path.
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
    // The Worker stays this realm's BTS Worker. What changes is that no reply
    // can come from it any more, so a disposal waiting for one settles now.
    if (backgroundWorker === worker) backgroundEnded = true;
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
      } else if (message.method === "disposed") {
        acknowledgeDisposal?.();
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
  // Snapshot initial data before queued events or render can mutate it.
  worker.postMessage({bobcat: "runtime", method: "initialize", ...__BobcatBackgroundData(data), systemInfo: SystemInfo, nativeModules: hostNativeModules});
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
  // An ended Worker gets no `dispose`: nothing over there could run it, and
  // no acknowledgement could come back. Terminating it again is a no-op.
  const ended = worker === undefined || backgroundEnded;
  backgroundDisposal = new Promise<void>(resolve => {
    if (ended) resolve();
    else acknowledgeDisposal = resolve;
  }).finally(() => {
    worker?.terminate();
    acknowledgeDisposal = undefined;
  });
  if (!ended) worker?.postMessage({ bobcat: "runtime", method: "dispose" });
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

/**
 * Reads a `<utf16Length>:<text>` record the native side wrote, the twin of
 * element-papi's own reader: the writer counted UTF-16 code units, so
 * `String.prototype.slice` takes each field without a scan and a field may
 * contain any character at all, the delimiter included. The writer is Bobcat,
 * so nothing here validates the payload.
 */
function splitRecord(record: string): string[] {
  const fields: string[] = [];
  let rest = record;
  while (rest !== "") {
    const separator = rest.indexOf(":");
    const units = Number(rest.slice(0, separator));
    const body = rest.slice(separator + 1);
    fields.push(body.slice(0, units));
    rest = body.slice(units);
  }
  return fields;
}

/**
 * The embedder's native modules, read once as this module evaluates: two
 * fields per module, its name then its method names joined with commas. This
 * realm only carries them to the BTS Worker, which is where `NativeModules`
 * lives — the MTS `NativeModules` stays `undefined`, as Lepus has none.
 */
const hostNativeModules: Record<string, string[]> = (() => {
  const fields = splitRecord(nativeModuleTable());
  const modules: Record<string, string[]> = {};
  for (let index = 0; index + 1 < fields.length; index += 2) {
    const methods = fields[index + 1]!;
    modules[fields[index]!] = methods === "" ? [] : methods.split(",");
  }
  return modules;
})();
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

/**
 * What a named Lepus chunk is called with: every binding the entry preamble
 * gives the entry, as a parameter of its own.
 *
 * Ordered, because the parameter list the chunk is compiled with and the
 * arguments it is applied to are both this list. Each value is read at the
 * call, not here: `__Card__` is a live `let`, and `SystemInfo` and
 * `__globalProps` are replaced outright by the host calls that update them.
 * The PAPI half is the `bobcat:element` namespace's own export names, so a
 * PAPI member added there reaches a chunk without a second list to update.
 */
const CHUNK_BINDINGS: [string, () => unknown][] = [
  ...Object.keys(elementPAPI).map((name): [string, () => unknown] =>
    [name, () => (elementPAPI as Record<string, unknown>)[name]]),
  ["__Card__", () => __Card__],
  ["lynx", () => lynx],
  ["console", () => console],
  ["SystemInfo", () => SystemInfo],
  ["__globalProps", () => __globalProps],
  ["NativeModules", () => NativeModules],
  ["_AddEventListener", () => _AddEventListener],
  ["_ReportError", () => _ReportError],
  ["_SetSourceMapRelease", () => _SetSourceMapRelease],
  ["__OnLifecycleEvent", () => __OnLifecycleEvent],
  ["__LoadLepusChunk", () => __LoadLepusChunk],
  ["__LoadStyleSheet", () => __LoadStyleSheet],
  ["__AdoptStyleSheet", () => __AdoptStyleSheet],
];

/** The parameter list the host compiles a chunk's body inside. */
const CHUNK_PARAMETERS = CHUNK_BINDINGS.map(([name]) => name).join(", ");

/** The values those parameters take, as of this call. */
function chunkArguments(): unknown[] {
  return CHUNK_BINDINGS.map(([, read]) => read());
}

/** A chunk body, as the host compiled it: one parameter per binding. */
type ChunkBody = (...bindings: unknown[]) => unknown;

/**
 * `__LoadLepusChunk`: load this page's chunk of that name and run it.
 *
 * The chunk is a script resource of its own, at the URL `chunkURL` builds and
 * `PageSource` registered it under, and it is loaded through the same
 * synchronous host loader a `require` uses — so the job this call runs in
 * parks until the host answers, and no other job of this realm runs meanwhile.
 * **Evaluated again on every call**, as native's `TemplateEntry` does: there
 * is no chunk cache here.
 *
 * A chunk does not share the entry's lexical scope. What it has is this
 * realm's `globalThis` and `CHUNK_BINDINGS` — the same values the entry
 * preamble imports — as the parameters of the function body it was compiled
 * as. A `var` at its top level is therefore local to that call.
 *
 * The answer is native's: `false` for a chunk this page does not carry and for
 * a different component entry, and `true` for a chunk that was found — even
 * when running it threw, which is reported and nothing more. A chunk that
 * does not *compile* was found too: the host answers a `SyntaxError`, where a
 * chunk it could not load at all is an ordinary `Error`.
 */
export function __LoadLepusChunk(path: string, options: {dynamicComponentEntry?: string; chunkType?: number}) {
  if (arguments.length < 2 || typeof path !== "string" || options === null || typeof options !== "object") {
    throw new TypeError("__LoadLepusChunk requires a string path and options object");
  }
  const entry = options.dynamicComponentEntry;
  if (typeof entry === "string" && cardURL(entry) !== __Card__) return false;
  let loaded;
  try {
    loaded = loadModuleSync(chunkURL(path, "__Card__"), CHUNK_PARAMETERS);
  } catch (error) {
    if (!(error instanceof SyntaxError)) return false;
    _ReportError(error);
    return true;
  }
  try { (loaded.value as ChunkBody).apply(undefined, chunkArguments()); }
  catch (error) { _ReportError(error); }
  return true;
}

export const NativeModules = undefined;

/**
 * The handle `__AdoptStyleSheet` takes: the URL the section was resolved to,
 * and nothing else. No realm state survives the call, so a handle is only
 * ever as good as the URL it names.
 */
export function __LoadStyleSheet(key: string, bundleName: string): {url: string} {
  if (arguments.length < 2 || typeof key !== "string" || typeof bundleName !== "string") {
    throw new TypeError("__LoadStyleSheet requires a section key and bundle name");
  }
  const url = styleSheetURL(key, bundleName);
  preloadStyleSheet(url);
  return {url};
}

export function __AdoptStyleSheet(handle: {url: string}) {
  const url = (handle as {url?: unknown} | null | undefined)?.url;
  if (typeof url !== "string") throw new TypeError("__AdoptStyleSheet requires a stylesheet handle");
  adoptStyleSheet(url);
  return null;
}

/**
 * This realm's `lynx.fetchBundle`.
 *
 * `later` is **inline**: on the main thread native runs a callback registered
 * on a handle whose value is already there then and there
 * (`LynxActor::Act`), which is what ReactLynx's `rLynxPrepareLazyBundleMTS`
 * depends on — its `loadScript('main-thread')` and `__LoadStyleSheet('CSS')`
 * have to run before the `callLepusMethod` reply reaches BTS.
 */
const bundleFetches = createBundleFetches({
  report: error => { _ReportError(error); },
  later: run => { run(); },
});

/**
 * `lynx.loadScript(key, {bundleName})`: one named custom section of a
 * container this realm has, evaluated once per realm.
 *
 * The load is the synchronous one a `require` is written over, of the URL the
 * section name builds beside the container's — the string `bobcat-source`
 * registered it under. The three shapes it can answer are `bobcat:module`'s
 * own: an **ES module**, which is what a body a container carried is, whose
 * default export is what native's host would have kept as that script's
 * completion value; **JSON**, the parsed value; and a plain **`CommonJS`**
 * file, compiled in `module, exports` alone, which answers `module.exports`.
 *
 * Unlike BTS's, this answers the value itself rather than an `{init}` object:
 * ReactLynx calls what it gets (`lynx.loadScript('main-thread', …)(entry)`),
 * and a `.lynx.bundle`'s `main-thread` body is the function expression it
 * calls.
 */
function loadScript(key: string, options: {bundleName?: string}): unknown {
  if (typeof key !== "string" || options === null || typeof options !== "object") {
    throw new TypeError("loadScript requires a section key and an options object");
  }
  const loaded = loadModuleSync(chunkURL(key, options.bundleName ?? "__Card__"), "module, exports");
  if (loaded.kind === "json") return loaded.value;
  if (loaded.kind === "module") {
    const namespace = loaded.value as {default?: unknown};
    return Object.hasOwn(namespace, "default") ? namespace.default : namespace;
  }
  const module = {exports: {} as unknown};
  (loaded.value as (module: unknown, exports: unknown) => void)
    .call(undefined, module, module.exports);
  return module.exports;
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
  // `options` is accepted and ignored, as native ignores it.
  fetchBundle(url: string, _options?: unknown): BundleHandle {
    return bundleFetches.fetchBundle(url, _options);
  },
  loadScript,
  triggerGlobalEventFromLepus: noop,
};
