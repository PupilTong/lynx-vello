import "bobcat:worker";
import type { WorkerGlobalScope } from "bobcat:worker";
import {
  type ContextEvent,
  createCrossThreadContext,
} from "bobcat:cross-thread-context";
import { SelectorQuery, type SendQuery } from "bobcat:selector-query";
import { requestScript } from "bobcat-internal:worker";
import { GlobalEventEmitter } from "bobcat:global-event-emitter";

// The bobcat:bts bootstrap and BTS applications import this runtime.
// Like MTS, lynx is a module binding, never a global property.
const scope = globalThis as unknown as WorkerGlobalScope;

const coreContext = createCrossThreadContext();

type AppHook = (...args: unknown[]) => unknown;
const emitter = new GlobalEventEmitter();
const jsModules = new Map<string, unknown>([["GlobalEventEmitter", emitter]]);
const app: {
  OnLifecycleEvent?: AppHook;
  publishEvent?: AppHook;
  publicComponentEvent?: AppHook;
  callDestroyLifetimeFun?: AppHook;
  updateGlobalProps?: AppHook;
  updateCardData?: AppHook;
  onAppReload?: AppHook;
  processCardConfig?: AppHook;
  _params: {initData: unknown; updateData: unknown; processorName: string; cacheData: unknown[]};
  GlobalEventEmitter: GlobalEventEmitter;
  registerModule(name: string, value: unknown): void;
  getJSModule(name: string): unknown;
} = {
  _params: {initData: null, updateData: undefined, processorName: "", cacheData: []},
  GlobalEventEmitter: emitter,
  registerModule(name, value) { jsModules.set(name, value); },
  getJSModule(name) { return jsModules.get(name); },
};
// Looked up by the id a `callLepusMethodResult` carries, which a result for
// a call made without a callback lacks.
const callbacks: Map<number | undefined, (result: unknown) => void> =
  new Map();
let nextCallbackId = 1;

/**
 * What the main thread sends this realm: a runtime call, tagged
 * `bobcat: "runtime"`, or a Context event's public fields, which carry no tag.
 */
type FromMainThread =
  | {
      bobcat: "runtime";
      method: "publishEvent" | "publicComponentEvent" | "updateGlobalProps" | "updateCardData" | "onAppReload" | "processCardConfig";
      args: unknown[];
    }
  | { bobcat: "runtime"; method: "callDestroyLifetimeFun" }
  | {
      bobcat: "runtime";
      method: "callLepusMethodResult";
      id?: number;
      result?: unknown;
      error?: { name: string; message: string };
    }
  | { bobcat: "runtime"; method: "nodeQueryResult" | "reloadResult"; id?: number; result?: unknown }
  | { bobcat: "runtime"; method: "sendGlobalEvent"; name: string; args: unknown[] }
  | (ContextEvent & { bobcat?: never });

/**
 * web-core registers these two handlers lazily: an event received before the
 * framework installs its hook waits for that hook. Later calls read the current
 * property because ReactLynx replaces the initial handler during setup.
 */
function createPublishHandler(name: "publishEvent" | "publicComponentEvent") {
  let handler: AppHook | undefined;
  let pending: unknown[][] = [];
  async function replay(args: unknown[]) {
    // As in web-core's async RPC receiver, call the current hook immediately,
    // but report a rejection independently so later queued events still run.
    return app[name]?.apply(app, args);
  }
  Object.defineProperty(app, name, {
    get() { return handler; },
    set(value: AppHook | undefined) {
      handler = value;
      if (typeof value === "function") {
        const queued = pending;
        pending = [];
        for (const args of queued) {
          void replay(args);
        }
      }
    },
  });
  return (args: unknown[]) => {
    const current = app[name];
    if (typeof current === "function") {
      current.apply(app, args);
    } else {
      pending.push(args);
    }
  };
}

const publishEvent = createPublishHandler("publishEvent");
const publicComponentEvent = createPublishHandler("publicComponentEvent");

const sendQuery: SendQuery = (operation, token, params, callback) => {
  // These are native BTS argument checks, before any asynchronous delivery.
  if (typeof token.identifier !== "string" || typeof token.component_id !== "string") {
    throw new TypeError("node identifier and component must be strings");
  }
  let id;
  if (callback) { id = nextCallbackId++; callbacks.set(id, callback); }
  try { scope.postMessage({bobcat: "runtime", method: "nodeQuery", operation, token, params, id}); }
  catch (error) { if (id !== undefined) callbacks.delete(id); throw error; }
};

type ScriptCallback = (error: string | null, source: string) => void;
const scriptCallbacks = new Map<string, ScriptCallback>();
let nextScriptId = 1;

/** Internal source transport; compiled module execution belongs to its caller. */
export function __BobcatRequestScript(path: string, callback: ScriptCallback) {
  const id = String(nextScriptId++);
  scriptCallbacks.set(id, callback);
  try { requestScript(id, path); }
  catch (error) { scriptCallbacks.delete(id); throw error; }
}

export function __BobcatCompleteScript(id: string, error: string | null, source: string) {
  const callback = scriptCallbacks.get(id);
  try { callback?.(error, source); }
  finally { scriptCallbacks.delete(id); }
}

const nativeApp = {
  callLepusMethod(
    name: string,
    data: unknown,
    callback?: (result: unknown) => void,
  ) {
    if (arguments.length < 2) throw new TypeError("callLepusMethod requires name and data");
    if (typeof name !== "string") name = "";
    if (data === null || typeof data !== "object") return;
    let id;
    if (typeof callback === "function") {
      id = nextCallbackId++;
      callbacks.set(id, callback);
    }
    try {
      scope.postMessage({
        bobcat: "runtime", method: "callLepusMethod", name, data, id,
      });
    } catch (error) {
      if (id !== undefined) callbacks.delete(id);
      throw error;
    }
  },
};

// web-worker-rpc callbackify invokes callbacks in a Promise continuation.
// Release the ID before scheduling it, so duplicate replies cannot invoke it twice.
async function receiveLepusResult(
  message: Extract<FromMainThread, { method: "callLepusMethodResult" }>,
) {
  const callback = callbacks.get(message.id);
  callbacks.delete(message.id);
  await undefined;
  if (message.error !== undefined) {
    const error = new Error(message.error.message);
    error.name = message.error.name;
    throw error;
  }
  return callback?.(message.result);
}

coreContext.addEventListener(
  "__OnLifecycleEvent",
  (event: { data: unknown }) => {
    app.OnLifecycleEvent?.call(app, event.data);
  },
);

coreContext.connect((event) => scope.postMessage({ type: event.type, data: event.data, origin: event.origin }));
scope.addEventListener("message", (event: { data: FromMainThread }): void | Promise<void> => {
  const message = event.data;
  if (message?.bobcat !== "runtime") {
    coreContext.receive(message);
    return;
  }
  switch (message.method) {
    case "publishEvent":
      publishEvent(message.args);
      break;
    case "publicComponentEvent":
      publicComponentEvent(message.args);
      break;
    case "callDestroyLifetimeFun":
      app.callDestroyLifetimeFun?.call(app);
      break;
    case "reloadResult":
    case "nodeQueryResult": {
      const callback = callbacks.get(message.id);
      try { callback?.(message.result); }
      finally { callbacks.delete(message.id); }
      break;
    }
    case "updateGlobalProps":
    case "updateCardData":
    case "onAppReload":
    case "processCardConfig":
      app[message.method]?.apply(app, message.args);
      break;
    case "sendGlobalEvent":
      emitter.emit(message.name, message.args);
      break;
    case "callLepusMethodResult":
      return receiveLepusResult(message);
  }
});

// The selected runtime target, independent of the compiler's minimum SDK.
export let SystemInfo: Readonly<Record<string, unknown>> = Object.freeze({
  platform: "headless", runtimeType: "quickjs", lynxSdkVersion: "4.1.0",
});

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
    (...args: unknown[]) => scope.postMessage({ bobcat: "runtime", method: "console", level,
      message: args.map(printable).join(" ") }),
  ]),
);

// This raw BTS environment supplies lifecycle inputs and hooks. Compiled
// factory/module bootstrap is a separate integration layer.
export const lynx = {
  reload(value?: unknown, callback?: unknown) {
    // Native only parses object arguments. Primitives (including null) mean
    // an empty data table; arrays/functions do not produce a reload table.
    let data = {};
    if (typeof value === "function") return;
    if (value !== null && typeof value === "object") {
      data = value;
      if (Array.isArray(data)) return;
    }
    let id;
    if (typeof callback === "function") {
      id = nextCallbackId++;
      callbacks.set(id, () => callback());
    }
    try { scope.postMessage({bobcat:"runtime", method:"reloadFromJS", data, id}); }
    catch (error) { if (id !== undefined) callbacks.delete(id); throw error; }
  },
  SystemInfo,
  __initData: {} as unknown,
  __globalProps: {} as unknown,
  getJSModule: app.getJSModule,
  registerModule: app.registerModule,
  reportError(error: unknown, options?: {level?: string}) {
    const level = options?.level;
    scope.postMessage({ bobcat: "runtime", method: "reportError",
      level: level === "warning" || level === "fatal" ? level : "error",
      message: printable(error) });
  },

  createSelectorQuery(component?: string) { return new SelectorQuery(sendQuery, error => lynx.reportError(error), component); },
  getApp() {
    return app;
  },
  getNativeApp() {
    return nativeApp;
  },
  getCoreContext() {
    return coreContext;
  },
};

interface BackgroundData {
  initData?: unknown;
  updateData?: unknown;
  globalProps?: unknown;
  processorName?: string;
  cacheData?: unknown[];
}

export function __BobcatInitializeBTS(options: BackgroundData & {
  backgroundData?: BackgroundData;
  systemInfo?: Record<string, unknown>;
}) {
  const params = options.backgroundData ?? options;
  app._params = { initData:params.initData ?? null, updateData:params.updateData, processorName:params.processorName ?? "", cacheData:params.cacheData ?? [] };
  lynx.__initData = Object.hasOwn(params, "updateData") ? params.updateData : params.initData;
  lynx.__globalProps = params.globalProps || {};
  if (options.systemInfo) SystemInfo = Object.freeze({ ...SystemInfo, ...options.systemInfo });
  lynx.SystemInfo = SystemInfo;
  // Keep the raw BTS environment consistent with its module snapshot.
  Object.assign(scope, { SystemInfo });
}

__BobcatInitializeBTS({});
